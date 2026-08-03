use super::*;

/// Project `$a0..$a3` at every proven-root-reachable direct or exhaustively
/// resolved indirect call. Values are sampled after the architectural delay
/// slot and before the unknown callee clobbers caller-owned state.
pub fn analyze_call_boundaries(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
) -> CallBoundaryAnalysisV1 {
    analyze_call_boundaries_from_roots(cfg, bank_bytes, va_start, &cfg.proven_roots)
}

/// `$a0..$a3` call-boundary projection rooted only at the supplied authority
/// entries. This prevents an unrelated callable root from joining away the
/// entry stub's symbolic stack identity.
pub fn analyze_call_boundaries_from_roots(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    analysis_roots: &[u32],
) -> CallBoundaryAnalysisV1 {
    analyze_call_boundary_registers_from_roots(
        cfg,
        bank_bytes,
        va_start,
        analysis_roots,
        &[4, 5, 6, 7],
    )
    .expect("the o32 argument-register set is valid")
}

/// Call-boundary projection for a caller-selected bounded register set.
/// Duplicate registers collapse and output is always register-sorted.
pub fn analyze_call_boundary_registers(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    requested_registers: &[u8],
) -> Result<CallBoundaryAnalysisV1, CallBoundaryAnalysisErrorV1> {
    analyze_call_boundary_registers_from_roots(
        cfg,
        bank_bytes,
        va_start,
        &cfg.proven_roots,
        requested_registers,
    )
}

/// Requested-register form rooted only at the supplied authority entries.
pub fn analyze_call_boundary_registers_from_roots(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    analysis_roots: &[u32],
    requested_registers: &[u8],
) -> Result<CallBoundaryAnalysisV1, CallBoundaryAnalysisErrorV1> {
    let requested_registers = requested_registers.iter().copied().collect::<BTreeSet<_>>();
    if let Some(register) = requested_registers
        .iter()
        .copied()
        .find(|register| *register >= 32)
    {
        return Err(CallBoundaryAnalysisErrorV1::InvalidRegister { register });
    }
    let requested_registers = requested_registers.into_iter().collect::<Vec<_>>();

    let mut callees = BTreeMap::new();
    for block in &cfg.blocks {
        let Some(site_pc) = block.end_va.checked_sub(8) else {
            continue;
        };
        let callee = match &block.terminator {
            BlockTerminator::Call { target, .. } => {
                Some(CallBoundaryCalleeV1::Direct { target: *target })
            }
            BlockTerminator::ResolvedIndirect {
                targets,
                via_call: true,
            } => {
                let targets = targets.iter().copied().collect::<BTreeSet<_>>();
                Some(CallBoundaryCalleeV1::ResolvedIndirect {
                    targets: targets.into_iter().collect(),
                })
            }
            _ => None,
        };
        if let Some(callee) = callee {
            callees.insert(site_pc, callee);
        }
    }

    let mut observations = BTreeMap::new();
    let _ = resolve_value_sets_from_roots_observing(
        cfg,
        bank_bytes,
        va_start,
        analysis_roots,
        None,
        None,
        None,
        Some((&requested_registers, &mut observations)),
    );

    let mut calls = Vec::new();
    for (site_pc, site_observations) in observations {
        let Some(callee) = callees.get(&site_pc).cloned() else {
            // Open indirect calls are observed by the shared engine but are
            // intentionally outside this exact-callee API.
            continue;
        };
        let registers = requested_registers
            .iter()
            .copied()
            .map(|register| reduce_call_register_observations(register, &site_observations))
            .collect();
        calls.push(CallBoundaryProofV1 {
            site_pc,
            callee,
            registers,
        });
    }
    Ok(CallBoundaryAnalysisV1 {
        requested_registers,
        calls,
    })
}

pub(super) fn reduce_call_register_observations(
    register: u8,
    observations: &BTreeSet<RawCallBoundaryObservation>,
) -> CallBoundaryRegisterProofV1 {
    let register_observations = observations
        .iter()
        .filter_map(|observation| {
            observation
                .registers
                .iter()
                .find(|candidate| candidate.register == register)
        })
        .collect::<Vec<_>>();
    let distinct = register_observations
        .iter()
        .map(|observation| (*observation).clone())
        .collect::<BTreeSet<_>>();
    let mut blockers = BTreeSet::new();
    if register_observations.is_empty() {
        blockers.insert(CallBoundaryValueBlockerV1::NoReachableObservation);
    }
    if observations.iter().any(|observation| observation.widened) {
        blockers.insert(CallBoundaryValueBlockerV1::RevisitWidened);
    }
    if distinct.len() > 1 {
        blockers.insert(CallBoundaryValueBlockerV1::PathDisagreement);
    }
    let mutable_sources = register_observations
        .iter()
        .filter(|observation| observation.from_static_memory)
        .flat_map(|observation| observation.memory_sources.iter().copied())
        .collect::<BTreeSet<_>>();
    if !mutable_sources.is_empty() {
        blockers.insert(CallBoundaryValueBlockerV1::MutableStaticMemorySource {
            addresses: mutable_sources.into_iter().collect(),
        });
    }

    let value = if distinct.len() == 1 {
        match &distinct.iter().next().unwrap().value {
            AbstractValue::Concrete(values) => CallBoundaryValueV1::Concrete {
                values: values.iter().copied().collect(),
            },
            AbstractValue::Stack { root, offsets } => CallBoundaryValueV1::StackLocations {
                root: *root,
                offsets: offsets.iter().copied().collect(),
            },
            AbstractValue::Unknown => {
                blockers.insert(CallBoundaryValueBlockerV1::ValueOpen);
                CallBoundaryValueV1::Open
            }
        }
    } else {
        blockers.insert(CallBoundaryValueBlockerV1::ValueOpen);
        CallBoundaryValueV1::Open
    };
    let memory_sources = register_observations
        .iter()
        .flat_map(|observation| observation.memory_sources.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let through_memory = register_observations
        .iter()
        .any(|observation| observation.through_memory);
    CallBoundaryRegisterProofV1 {
        register,
        value,
        memory_sources,
        through_memory,
        blockers: blockers.into_iter().collect(),
    }
}

/// Inventory every aligned `MTC0`/`DMTC0` write to COP0 Status in `bank_bytes`.
///
/// The raw image scan is exhaustive for the supplied bytes. The accompanying
/// [`Cfg`] contributes only the word classification and open-indirect
/// frontier; this function never promotes candidate or unknown words to code.
pub fn inventory_cop0_status_writes(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<Cop0StatusWriteInventory, Cop0StatusWriteInventoryError> {
    if !bank_bytes.len().is_multiple_of(4) {
        return Err(Cop0StatusWriteInventoryError::UnalignedImage);
    }
    let byte_len = u32::try_from(bank_bytes.len())
        .map_err(|_| Cop0StatusWriteInventoryError::AddressOverflow)?;
    va_start
        .checked_add(byte_len)
        .ok_or(Cop0StatusWriteInventoryError::AddressOverflow)?;
    let mut proven_code_writes = Vec::new();
    let mut proven_data_words = Vec::new();
    let mut unclassified_writes = Vec::new();

    for (index, bytes) in bank_bytes.chunks_exact(4).enumerate() {
        let site_pc = va_start + index as u32 * 4;
        let instruction_word = u32::from_be_bytes(bytes.try_into().unwrap());
        let (source_register, kind) = match decode(instruction_word) {
            Instruction::Mtc0 { rt, cop0d: 12 } => (rt, Cop0StatusWriteKind::Mtc0),
            Instruction::Dmtc0 { rt, cop0d: 12 } => (rt, Cop0StatusWriteKind::Dmtc0),
            _ => continue,
        };
        let word_class = cfg.word_class.get(&site_pc).copied();
        let site = Cop0StatusWriteSite {
            site_pc,
            instruction_word,
            source_register,
            kind,
            word_class,
        };
        match word_class {
            Some(WordClass::ProvenCode) => proven_code_writes.push(site),
            Some(WordClass::ProvenData) => proven_data_words.push(site),
            Some(WordClass::Unknown)
            | Some(WordClass::CandidateData)
            | Some(WordClass::CandidateCode)
            | Some(WordClass::Conflict)
            | None => unclassified_writes.push(site),
        }
    }

    let mut open_indirect_sites = cfg
        .indirect_sites
        .iter()
        .map(|site| site.pc)
        .collect::<Vec<_>>();
    open_indirect_sites.sort_unstable();
    open_indirect_sites.dedup();
    Ok(Cop0StatusWriteInventory {
        proven_code_writes,
        proven_data_words,
        unclassified_writes,
        open_indirect_sites,
    })
}

/// Prove the bounded source-GPR value set at every proven-code Status write.
///
/// This reuses the same whole-CFG abstract state as indirect-target and fixed
/// store resolution. It samples values before the write executes, rejects
/// mutable load-image provenance, and never treats the 32-bit domain as proof
/// for `DMTC0`'s 64-bit operand.
pub fn analyze_cop0_status_writes(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<Cop0StatusWriteAnalysis, Cop0StatusWriteInventoryError> {
    let inventory = inventory_cop0_status_writes(cfg, bank_bytes, va_start)?;
    let mut observations = BTreeMap::new();
    let _ = resolve_value_sets_from_roots_observing(
        cfg,
        bank_bytes,
        va_start,
        &cfg.proven_roots,
        None,
        Some(&mut observations),
        None,
        None,
    );
    let mut proofs = Vec::with_capacity(inventory.proven_code_writes.len());
    for site in &inventory.proven_code_writes {
        let site_observations = observations.remove(&site.site_pc).unwrap_or_default();
        let mut blockers = BTreeSet::new();
        if site_observations.is_empty() {
            blockers.insert(Cop0StatusValueBlocker::NoReachableObservation);
        }
        if site.kind == Cop0StatusWriteKind::Dmtc0 {
            blockers.insert(Cop0StatusValueBlocker::Dmtc0Unsupported);
        }
        if site_observations
            .iter()
            .any(|observation| observation.values.is_none())
        {
            blockers.insert(Cop0StatusValueBlocker::ValueOpen);
        }
        if site_observations
            .iter()
            .any(|observation| observation.widened)
        {
            blockers.insert(Cop0StatusValueBlocker::RevisitWidened);
        }
        let mutable_sources = site_observations
            .iter()
            .filter(|observation| observation.from_static_memory)
            .flat_map(|observation| observation.memory_sources.iter().copied())
            .collect::<BTreeSet<_>>();
        if site_observations
            .iter()
            .any(|observation| observation.from_static_memory)
        {
            blockers.insert(Cop0StatusValueBlocker::MutableStaticMemorySource {
                addresses: mutable_sources.into_iter().collect(),
            });
        }
        let observed_values = site_observations
            .iter()
            .filter_map(|observation| observation.values.as_ref())
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>();
        let values = if observed_values.len() > MAX_VALUE_SET {
            blockers.insert(Cop0StatusValueBlocker::ValueSetOverflow {
                observed: u32::try_from(observed_values.len()).unwrap_or(u32::MAX),
            });
            Vec::new()
        } else {
            observed_values.into_iter().collect::<Vec<_>>()
        };
        let known_zero = site_observations
            .iter()
            .map(|observation| observation.known_zero)
            .reduce(|left, right| left & right)
            .unwrap_or(0);
        let known_one = site_observations
            .iter()
            .map(|observation| observation.known_one)
            .reduce(|left, right| left & right)
            .unwrap_or(0);
        proofs.push(Cop0StatusValueProof {
            site_pc: site.site_pc,
            values,
            known_zero,
            known_one,
            blockers: blockers.into_iter().collect(),
        });
    }
    Ok(Cop0StatusWriteAnalysis {
        inventory,
        proven_code_value_proofs: proofs,
    })
}

/// Correlate each reachable computed transfer with the exact indexed TLB
/// writes active after its delay slot.
///
/// This proves only path-invariant raw setup and target values. Consumers must
/// ask the execution runtime to translate the target and must independently
/// establish backing bytes before admitting an executable address view.
pub fn analyze_constant_tlb_transfers(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
) -> ConstantTlbTransferAnalysisV1 {
    let mut observations = BTreeMap::new();
    let _ = resolve_value_sets_from_roots_observing(
        cfg,
        bank_bytes,
        va_start,
        &cfg.proven_roots,
        None,
        None,
        Some(&mut observations),
        None,
    );

    let mut sites = cfg
        .indirect_sites
        .iter()
        .map(|site| site.pc)
        .collect::<BTreeSet<_>>();
    sites.extend(observations.keys().copied());
    let transfers = sites
        .into_iter()
        .map(|transfer_pc| {
            let site_observations = observations.remove(&transfer_pc).unwrap_or_default();
            let mut blockers = BTreeSet::new();
            if site_observations.is_empty() {
                blockers.insert(TlbTransferBlockerV1::NoReachableObservation);
            }
            if site_observations
                .iter()
                .any(|observation| observation.target.via_call)
            {
                blockers.insert(TlbTransferBlockerV1::ViaCall);
            }
            if site_observations.iter().any(|observation| {
                observation.target.state != IndirectProofState::Exhaustive
                    || observation.target.kind != Some(IndirectResolutionKind::Constant)
                    || observation.target.targets.len() != 1
            }) {
                blockers.insert(TlbTransferBlockerV1::TargetOpen);
            }
            if site_observations
                .iter()
                .any(|observation| observation.widened)
            {
                blockers.insert(TlbTransferBlockerV1::RevisitWidened);
            }
            for blocker in site_observations
                .iter()
                .flat_map(|observation| observation.blockers.iter().cloned())
            {
                blockers.insert(blocker);
            }

            let targets = site_observations
                .iter()
                .filter_map(|observation| {
                    (observation.target.state == IndirectProofState::Exhaustive
                        && observation.target.kind == Some(IndirectResolutionKind::Constant)
                        && observation.target.targets.len() == 1)
                        .then(|| observation.target.targets[0])
                })
                .collect::<BTreeSet<_>>();
            let target = (targets.len() == 1).then(|| *targets.iter().next().unwrap());
            if targets.len() > 1 {
                blockers.insert(TlbTransferBlockerV1::TargetPathDisagreement);
            }

            let active_write_sets = site_observations
                .iter()
                .map(|observation| observation.active_writes.clone())
                .collect::<BTreeSet<_>>();
            let active_writes = if active_write_sets.len() == 1 {
                active_write_sets.iter().next().cloned().unwrap_or_default()
            } else {
                blockers.insert(TlbTransferBlockerV1::TlbPathDisagreement);
                Vec::new()
            };
            if active_writes.is_empty() {
                blockers.insert(TlbTransferBlockerV1::NoProvenTlbWrite);
            }

            let entry_hi_values = site_observations
                .iter()
                .filter_map(|observation| observation.entry_hi_at_transfer)
                .collect::<BTreeSet<_>>();
            let entry_hi_at_transfer =
                (entry_hi_values.len() == 1).then(|| *entry_hi_values.iter().next().unwrap());
            if site_observations
                .iter()
                .any(|observation| observation.entry_hi_at_transfer.is_none())
                || entry_hi_values.len() > 1
            {
                blockers.insert(TlbTransferBlockerV1::EntryHiPathDisagreement);
            }

            TlbTransferProofV1 {
                transfer_pc,
                target,
                entry_hi_at_transfer,
                active_writes,
                blockers: blockers.into_iter().collect(),
            }
        })
        .collect();
    ConstantTlbTransferAnalysisV1 { transfers }
}

/// Derive exact fixed-address word stores whose value is an unchanged load of
/// an admitted load-image word.
///
/// Results are conditional on source stability; this pass proves the bounded
/// instruction data flow, not that no earlier CPU/device writer changed an
/// admitted source word.  Every reachable `sw` with an unknown address remains
/// in `open`, because it may alias a watched destination.  Concrete stores
/// wholly outside `watched_destinations` are omitted.
pub fn derive_fixed_word_stores(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    watched_destinations: &[u32],
    admitted_sources: &[AdmittedWordSource],
) -> Result<FixedWordStoreReport, FixedWordStoreInputError> {
    let mut watched = BTreeSet::new();
    for &address in watched_destinations {
        if !address.is_multiple_of(4) {
            return Err(FixedWordStoreInputError::UnalignedWatchedDestination { address });
        }
        watched.insert(address);
    }

    let mut admitted = BTreeMap::new();
    for source in admitted_sources {
        if !source.address.is_multiple_of(4) {
            return Err(FixedWordStoreInputError::UnalignedSource {
                address: source.address,
            });
        }
        if let Some(first) = admitted.insert(source.address, source.value) {
            if first != source.value {
                return Err(FixedWordStoreInputError::ConflictingSourceValues {
                    address: source.address,
                    first,
                    second: source.value,
                });
            }
        }
    }

    let mut observations = BTreeMap::new();
    let _ = resolve_value_sets_from_roots_observing(
        cfg,
        bank_bytes,
        va_start,
        &cfg.proven_roots,
        Some(&mut observations),
        None,
        None,
        None,
    );

    let mut conditional = Vec::new();
    let mut open = Vec::new();
    for (site_pc, site_observations) in observations {
        let address_is_open = site_observations
            .iter()
            .any(|observation| observation.addresses.is_none());
        let addresses: BTreeSet<u32> = site_observations
            .iter()
            .filter_map(|observation| observation.addresses.as_ref())
            .flatten()
            .copied()
            .collect();
        if !address_is_open && addresses.is_disjoint(&watched) {
            continue;
        }

        let mut blockers = BTreeSet::new();
        if site_observations.len() > 1 {
            blockers.insert(FixedWordStoreBlocker::PathDisagreement);
        }
        if site_observations
            .iter()
            .any(|observation| observation.widened)
        {
            blockers.insert(FixedWordStoreBlocker::RevisitWidened);
        }

        let destination = if address_is_open {
            blockers.insert(FixedWordStoreBlocker::AddressOpen);
            None
        } else if addresses.len() == 1 && !addresses.is_disjoint(&watched) {
            addresses.iter().next().copied()
        } else {
            blockers.insert(FixedWordStoreBlocker::AddressSetAmbiguous {
                addresses: addresses.iter().copied().collect(),
            });
            None
        };

        let value_is_open = site_observations
            .iter()
            .any(|observation| observation.values.is_none());
        let values: BTreeSet<u32> = site_observations
            .iter()
            .filter_map(|observation| observation.values.as_ref())
            .flatten()
            .copied()
            .collect();
        let value = if value_is_open {
            blockers.insert(FixedWordStoreBlocker::ValueOpen);
            None
        } else if values.len() == 1 {
            values.iter().next().copied()
        } else {
            blockers.insert(FixedWordStoreBlocker::ValueSetAmbiguous {
                values: values.iter().copied().collect(),
            });
            None
        };

        let sources: BTreeSet<u32> = site_observations
            .iter()
            .filter_map(|observation| observation.unchanged_static_word_source)
            .collect();
        let every_observation_has_source = site_observations
            .iter()
            .all(|observation| observation.unchanged_static_word_source.is_some());
        let source_address = if every_observation_has_source && sources.len() == 1 {
            sources.iter().next().copied()
        } else {
            blockers.insert(FixedWordStoreBlocker::ValueNotUnchangedStaticLoad);
            None
        };

        let source = source_address.and_then(|address| match admitted.get(&address).copied() {
            None => {
                blockers.insert(FixedWordStoreBlocker::SourceNotAdmitted { address });
                None
            }
            Some(admitted_value) if Some(admitted_value) != value => {
                if let Some(recovered) = value {
                    blockers.insert(FixedWordStoreBlocker::SourceValueMismatch {
                        address,
                        admitted: admitted_value,
                        recovered,
                    });
                }
                None
            }
            Some(value) => Some(AdmittedWordSource { address, value }),
        });

        if blockers.is_empty() {
            conditional.push(ConditionalFixedWordStore {
                site_pc,
                destination: destination.expect("empty blockers require one destination"),
                value: value.expect("empty blockers require one value"),
                source: source.expect("empty blockers require one admitted source"),
            });
        } else {
            open.push(OpenFixedWordStore {
                site_pc,
                blockers: blockers.into_iter().collect(),
            });
        }
    }

    Ok(FixedWordStoreReport { conditional, open })
}

/// How many dominating predecessor blocks a backward slice may prepend before
/// giving up. angr's MIPS resolver walks at most one predecessor (its
/// "two-block" case, `mips_elf_fast.py`, BSD-2); this generalizes the same idea
/// to a short linear chain because AKI's address-materialization commonly
/// spans a `lui` in the function prologue, a `move`/`addiu` in a body block, and
/// the `jr` in a third. Each step is admitted only when the predecessor is
/// *unique*, so the sliced entry state is genuinely the abstract top -- a longer
/// chain can only stay open or resolve, never fabricate.
pub(super) const MAX_BACKSLICE_DEPTH: usize = 4;

/// Map every reachable block start to its predecessor block starts, over the
/// same successor relation the forward pass walks. Only edges between blocks
/// the CFG actually contains are recorded (an out-of-bank successor cannot be a
/// slice ancestor).
pub(super) fn predecessor_map(cfg: &Cfg) -> BTreeMap<u32, BTreeSet<u32>> {
    let block_starts: BTreeSet<u32> = cfg.blocks.iter().map(|block| block.start_va).collect();
    let mut predecessors: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for block in &cfg.blocks {
        for successor in block_successors(block) {
            if block_starts.contains(&successor) {
                predecessors
                    .entry(successor)
                    .or_default()
                    .insert(block.start_va);
            }
        }
    }
    predecessors
}

/// The linear chain of blocks that unconditionally dominates `site_block`,
/// deepest ancestor first, ending at `site_block`. Each ancestor is included
/// only when the block below it has exactly one predecessor: with a unique
/// predecessor the entry state is unambiguous, so executing the ancestor's
/// exit state into the successor is the block's *only* possible entry -- no
/// merged path can weaken it. A block with zero or multiple predecessors ends
/// the chain (we cannot prove a single dominating construction beyond it).
pub(super) fn dominating_linear_chain(
    site_start: u32,
    predecessors: &BTreeMap<u32, BTreeSet<u32>>,
    blocks: &BTreeMap<u32, &BasicBlock>,
) -> Vec<u32> {
    let mut chain = vec![site_start];
    let mut current = site_start;
    let mut seen: BTreeSet<u32> = BTreeSet::from([site_start]);
    while chain.len() < MAX_BACKSLICE_DEPTH {
        let Some(preds) = predecessors.get(&current) else {
            break;
        };
        if preds.len() != 1 {
            break;
        }
        let pred = *preds.iter().next().unwrap();
        // A self-loop or an already-visited block would make the "chain" a
        // cycle whose entry state is not a single dominating construction.
        if !seen.insert(pred) || !blocks.contains_key(&pred) {
            break;
        }
        // The predecessor must fall straight into this block: if it ends in a
        // conditional branch or a call, its exit register file is not the
        // guaranteed entry state (the branch may refine one edge, the call
        // clobbers). Only an unconditional single-successor terminator gives a
        // sound "the block below is entered with exactly this state".
        let pred_block = blocks[&pred];
        if !matches!(
            pred_block.terminator,
            BlockTerminator::Fallthrough { .. } | BlockTerminator::Tail { .. }
        ) {
            break;
        }
        chain.push(pred);
        current = pred;
    }
    chain.reverse();
    chain
}

/// Concatenate a dominating chain's words (deepest ancestor first) up to but
/// NOT including the site's transfer word, then run the same bounded abstract
/// interpreter the forward pass uses over that straight-line slice from a clean
/// abstract-top state. This is the angr backward-slice technique
/// (`mips_elf_fast.py`, BSD-2, reimplemented in fn64's own value-set style):
/// re-derive the transfer register's construction locally, free of the global
/// fixpoint's revisit-widening and multi-predecessor join dilution that leave a
/// genuinely-constant `jr`/`jalr` open. Because the entry state is top and the
/// chain is unconditionally dominating, any value that closes here is
/// constructed on *every* path that reaches the site.
pub(super) fn backslice_site_value(
    chain: &[u32],
    site_pc: u32,
    transfer_register: u8,
    blocks: &BTreeMap<u32, &BasicBlock>,
    bank_bytes: &[u8],
    va_start: u32,
) -> TrackedValue {
    // Start from abstract top (every register Unknown). Unlike `at_root`, we do
    // not even assume a stack root: the slice must build the target from words
    // it actually contains, or the register stays Unknown and the site is left
    // open. `$zero` is pinned by `widened`.
    let mut state = AnalysisState::widened();
    for &block_start in chain {
        let Some(block) = blocks.get(&block_start) else {
            return TrackedValue::unknown();
        };
        for (pc, word) in read_block_words(block, bank_bytes, va_start) {
            // Stop exactly at the transfer word: on MIPS the register is read
            // when the jump issues, so neither it nor its delay slot can change
            // the resolved target.
            if pc == site_pc {
                return state.registers[transfer_register as usize].clone();
            }
            execute_instruction(&mut state, pc, word, bank_bytes, va_start);
        }
    }
    // The site's transfer word was never reached (the site block was not the
    // chain tail, or the words ran short): no proof.
    TrackedValue::unknown()
}

/// Upgrade indirect sites the forward pass left `Open` by backward-slicing each
/// one's transfer register through its unconditionally-dominating linear block
/// chain (angr `mips_elf_fast.py` technique, BSD-2, reimplemented). Only sites
/// that are still `Open` are touched, and only ever *toward* a proof: a slice
/// that closes to a finite in-bank set becomes `Exhaustive`/`Bounded` via the
/// shared `resolution_from_value`; a slice that stays Unknown leaves the site
/// exactly as it was. The verdict is therefore monotone -- the backslice can
/// never demote a forward proof or invent a target the interpreter would not
/// also accept forward.
pub(super) fn backslice_open_sites(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    resolutions: &mut [IndirectResolution],
) {
    let blocks: BTreeMap<u32, &BasicBlock> = cfg
        .blocks
        .iter()
        .map(|block| (block.start_va, block))
        .collect();
    let predecessors = predecessor_map(cfg);

    for resolution in resolutions.iter_mut() {
        if resolution.state != IndirectProofState::Open {
            continue;
        }
        // Locate the block whose transfer word is this site.
        let Some(block) = cfg.blocks.iter().find(|block| {
            resolution.site_pc >= block.start_va && resolution.site_pc < block.end_va
        }) else {
            continue;
        };
        // The site must be the block's own indirect transfer word.
        if block.end_va.checked_sub(8) != Some(resolution.site_pc) {
            continue;
        }
        let Some((transfer_register, via_call)) =
            terminator_register(bank_bytes, va_start, block.end_va)
        else {
            continue;
        };
        if via_call != resolution.via_call {
            continue;
        }
        let chain = dominating_linear_chain(block.start_va, &predecessors, &blocks);
        let value = backslice_site_value(
            &chain,
            resolution.site_pc,
            transfer_register,
            &blocks,
            bank_bytes,
            va_start,
        );
        let candidate = resolution_from_value(resolution.site_pc, via_call, &value);
        // Only ever accept a strictly-proving verdict; never overwrite the Open
        // record with another Open (that would erase the site's frontier note),
        // and never with a Bounded that carries no usable evidence.
        if candidate.state == IndirectProofState::Exhaustive {
            *resolution = candidate;
        }
    }
}

/// Read the register a `jr`/`jalr` terminator transfers through, directly
/// from the terminator instruction word at `end_va - 8` (the transfer word;
/// its delay slot is at `end_va - 4`). Returns `(rs, via_call)`.
pub(super) fn terminator_register(bank_bytes: &[u8], va_start: u32, end_va: u32) -> Option<(u8, bool)> {
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
