use super::*;

/// A proven callable transfer in one bank whose target lands inside another
/// bank's proven VA range.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct CrossBankAuthoritativeCall {
    source_bank: String,
    source_pc: u32,
    target_pc: u32,
    kind: CrossBankAuthoritativeCallKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CrossBankAuthoritativeCallKind {
    Direct,
    ExhaustiveResolved,
}

#[derive(Debug)]
pub(super) struct BankInterval {
    pub(super) input_index: usize,
    pub(super) bank: String,
    pub(super) va_start: u32,
    pub(super) va_end: u32,
}

/// Static point-query index over prepared bank ranges. A balanced max-end tree
/// prunes subtrees whose intervals all end before the target; overlapping
/// ranges are all returned in input order.
pub(super) struct BankIntervalIndex {
    intervals: Vec<BankInterval>,
    leaf_base: usize,
    max_end_tree: Vec<u32>,
}

impl BankIntervalIndex {
    fn new(prepared: &[PreparedBank]) -> Self {
        Self::from_intervals(
            prepared
                .iter()
                .enumerate()
                .map(|(input_index, bank)| BankInterval {
                    input_index,
                    bank: bank.bank.clone(),
                    va_start: bank.va_start,
                    va_end: bank.va_end,
                })
                .collect(),
        )
    }

    pub(super) fn from_intervals(mut intervals: Vec<BankInterval>) -> Self {
        intervals.sort_by(|left, right| {
            (
                left.va_start,
                left.va_end,
                left.bank.as_str(),
                left.input_index,
            )
                .cmp(&(
                    right.va_start,
                    right.va_end,
                    right.bank.as_str(),
                    right.input_index,
                ))
        });
        let leaf_base = intervals.len().next_power_of_two().max(1);
        let mut max_end_tree = vec![0; leaf_base * 2];
        for (index, interval) in intervals.iter().enumerate() {
            max_end_tree[leaf_base + index] = interval.va_end;
        }
        for node in (1..leaf_base).rev() {
            max_end_tree[node] = max_end_tree[node * 2].max(max_end_tree[node * 2 + 1]);
        }
        Self {
            intervals,
            leaf_base,
            max_end_tree,
        }
    }

    pub(super) fn matching_other_banks(&self, source_bank: &str, target_pc: u32) -> Vec<usize> {
        self.matching_other_banks_with_probe_count(source_bank, target_pc)
            .0
    }

    pub(super) fn matching_other_banks_with_probe_count(
        &self,
        source_bank: &str,
        target_pc: u32,
    ) -> (Vec<usize>, usize) {
        let upper = self
            .intervals
            .partition_point(|interval| interval.va_start <= target_pc);
        let mut matches = Vec::new();
        let mut probes = 0;
        self.query_node(
            1,
            0,
            self.leaf_base,
            upper,
            source_bank,
            target_pc,
            &mut matches,
            &mut probes,
        );
        matches.sort_unstable();
        (matches, probes)
    }

    #[allow(clippy::too_many_arguments)]
    fn query_node(
        &self,
        node: usize,
        range_start: usize,
        range_end: usize,
        upper: usize,
        source_bank: &str,
        target_pc: u32,
        matches: &mut Vec<usize>,
        probes: &mut usize,
    ) {
        if range_start >= upper || self.max_end_tree[node] <= target_pc {
            return;
        }
        if range_end - range_start == 1 {
            *probes += 1;
            if let Some(interval) = self.intervals.get(range_start) {
                if interval.va_end > target_pc && interval.bank != source_bank {
                    matches.push(interval.input_index);
                }
            }
            return;
        }
        let midpoint = range_start + (range_end - range_start) / 2;
        self.query_node(
            node * 2,
            range_start,
            midpoint,
            upper,
            source_bank,
            target_pc,
            matches,
            probes,
        );
        self.query_node(
            node * 2 + 1,
            midpoint,
            range_end,
            upper,
            source_bank,
            target_pc,
            matches,
            probes,
        );
    }
}

#[derive(Default)]
pub(super) struct SerializedByteCounter(u64);

impl Write for SerializedByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let byte_len = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("serialized byte count exceeds u64"))?;
        self.0 = self
            .0
            .checked_add(byte_len)
            .ok_or_else(|| io::Error::other("serialized byte count exceeds u64"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn check_multi_bank_limits<'a>(
    base_facts: &'a FactDb,
    inputs: &[MaterializedBankInput<'_>],
    limits: MultiBankCompositionLimits,
) -> Result<FactProjectionIndex<'a>, SnapshotError> {
    let projection_index =
        FactProjectionIndex::new(base_facts).map_err(SnapshotError::FactProjection)?;
    let total_banks = u64::try_from(inputs.len()).map_err(|_| {
        SnapshotError::CompositionLimitArithmeticOverflow {
            calculation: "bank count conversion",
        }
    })?;
    let global_fact_rows = u64::try_from(projection_index.global_fact_count()).map_err(|_| {
        SnapshotError::CompositionLimitArithmeticOverflow {
            calculation: "global fact row count conversion",
        }
    })?;
    let mut projected_rows = 0u64;
    let mut projected_bytes = 0u64;
    for (bank_index, input) in inputs.iter().enumerate() {
        let projected = projection_index.project(input.bank);
        let rows = u64::try_from(projected.facts().len()).map_err(|_| {
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "projected fact row count conversion",
            }
        })?;
        projected_rows = projected_rows.checked_add(rows).ok_or(
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "aggregate projected fact rows",
            },
        )?;
        if projected_rows > limits.max_projected_fact_rows {
            return Err(SnapshotError::ProjectedFactRowsLimitExceeded {
                rows: projected_rows,
                limit: limits.max_projected_fact_rows,
            });
        }

        let mut serialized = SerializedByteCounter::default();
        serde_json::to_writer(&mut serialized, &projected).map_err(|error| {
            SnapshotError::ProjectedFactsSerialization {
                bank: input.bank.to_owned(),
                error: error.to_string(),
            }
        })?;
        projected_bytes = projected_bytes.checked_add(serialized.0).ok_or(
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "aggregate projected fact bytes",
            },
        )?;
        if projected_bytes > limits.max_projected_fact_bytes {
            let processed_banks = u64::try_from(bank_index + 1).map_err(|_| {
                SnapshotError::CompositionLimitArithmeticOverflow {
                    calculation: "processed bank count conversion",
                }
            })?;
            let bank_scoped_rows = u64::try_from(projection_index.scoped_fact_count(input.bank))
                .map_err(|_| SnapshotError::CompositionLimitArithmeticOverflow {
                    calculation: "bank-scoped fact row count conversion",
                })?;
            let bank_selected_conclusions = u64::try_from(
                projection_index.selected_conclusion_count(input.bank),
            )
            .map_err(|_| SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "selected conclusion count conversion",
            })?;
            return Err(SnapshotError::ProjectedFactBytesLimitExceeded {
                bank: input.bank.to_owned(),
                bytes: projected_bytes,
                rows: projected_rows,
                bank_rows: rows,
                bank_scoped_rows,
                bank_selected_conclusions,
                largest_justifications: projection_index
                    .largest_selected_justifications(input.bank, 5),
                processed_banks,
                total_banks,
                global_fact_rows,
                limit: limits.max_projected_fact_bytes,
            });
        }
    }
    if std::env::var_os(REPORT_PROJECTION_STATS_ENV).is_some() {
        eprintln!(
            "fn64 projection-stats banks={total_banks} rows={projected_rows} bytes={projected_bytes} global_rows_per_bank={global_fact_rows}"
        );
    }

    let materialized_bytes = inputs.iter().try_fold(0u64, |total, input| {
        let bytes = u64::try_from(input.bytes.len()).map_err(|_| {
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "materialized byte count conversion",
            }
        })?;
        total
            .checked_add(bytes)
            .ok_or(SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "aggregate materialized bytes",
            })
    })?;
    if materialized_bytes > limits.max_aggregate_materialized_bytes {
        return Err(SnapshotError::AggregateMaterializedBytesLimitExceeded {
            bytes: materialized_bytes,
            limit: limits.max_aggregate_materialized_bytes,
        });
    }
    Ok(projection_index)
}

pub(super) fn validate_unique_bank_names(inputs: &[MaterializedBankInput<'_>]) -> Result<(), SnapshotError> {
    let mut names = BTreeSet::new();
    for input in inputs {
        if input.bank.is_empty() || input.bank.trim() != input.bank {
            return Err(SnapshotError::InvalidBankName);
        }
        if !names.insert(input.bank) {
            return Err(SnapshotError::DuplicateBankName {
                bank: input.bank.to_owned(),
            });
        }
    }
    Ok(())
}

pub(super) fn insert_cross_bank_authority(
    cross_calls: &mut BTreeMap<String, BTreeSet<CrossBankAuthoritativeCall>>,
    target_bank: &str,
    call: CrossBankAuthoritativeCall,
    record_count: &mut u64,
    limit: u64,
) -> Result<(), SnapshotError> {
    if cross_calls
        .entry(target_bank.to_owned())
        .or_default()
        .insert(call)
    {
        *record_count = record_count.checked_add(1).ok_or(
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "cross-bank authority record count",
            },
        )?;
        if *record_count > limit {
            return Err(SnapshotError::CrossBankAuthorityRecordsLimitExceeded {
                records: *record_count,
                limit,
            });
        }
    }
    Ok(())
}

pub(super) fn build_cross_bank_authority_closure(
    base_facts: &FactDb,
    bank: &str,
    bytes: &[u8],
    va_start: u32,
    authorized_callable_roots: &BTreeSet<u32>,
    cross_bank_reachability_roots: &BTreeSet<u32>,
) -> ClosureResult {
    let mut roots: BTreeSet<u32> = base_facts
        .proven_function_entries(bank)
        .into_iter()
        .collect();
    roots.extend(authorized_callable_roots.iter().copied());
    roots.extend(cross_bank_reachability_roots.iter().copied());
    build_cfg_value_set_closed(
        bank,
        bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    )
}

pub(super) fn authority_reachable_direct_calls(source: &PreparedBank) -> BTreeSet<(u32, u32)> {
    let cfg = &source.authority_closure.cfg;
    cfg.blocks
        .iter()
        .filter_map(|block| exact_authority_direct_call(cfg, block))
        .collect()
}

pub(super) fn authority_reachable_direct_jumps(source: &PreparedBank) -> BTreeSet<(u32, u32)> {
    let cfg = &source.authority_closure.cfg;
    cfg.blocks
        .iter()
        .filter_map(|block| {
            let crate::cfg::BlockTerminator::Tail { target } = &block.terminator else {
                return None;
            };
            let source_pc = block.end_va.checked_sub(8)?;
            (cfg.word_class.get(&source_pc) == Some(&crate::cfg::WordClass::ProvenCode)
                && cfg.word_class.get(&(source_pc + 4)) == Some(&crate::cfg::WordClass::ProvenCode)
                && cfg.tail_transfers.contains(&(source_pc, *target)))
            .then_some((source_pc, *target))
        })
        .collect()
}

/// Compose several byte-verified banks together, letting a proven direct `jal`
/// in any one bank confer callable-entry authority on the target bank it lands
/// in. Returns one [`ProgramSnapshotV1`] per input bank, in input order.
///
/// This is the multi-bank counterpart to [`compose_materialized_bank_v1`]. Each
/// bank is prepared (validated, byte-verified, closure-built) exactly as in the
/// single-bank path; the only added authority is cross-bank. A direct call
/// from proven code, or a computed call whose typed analysis is
/// exhaustive and exactly matches its CFG terminator, whose target lands
/// aligned inside bank Y's proven VA range becomes an authoritative callable
/// root of bank Y. These are the identical two authority rules already used
/// for same-bank calls, extended across the catalog boundary. Open/bounded
/// computed calls and tail transfers never confer authority. A bank composed
/// alone here (no siblings) is byte-identical to
/// `compose_materialized_bank_v1`.
pub fn compose_materialized_banks_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
) -> Result<Vec<ProgramSnapshotV1>, SnapshotError> {
    compose_materialized_banks_v1_with_limits(
        rom,
        base_facts,
        inputs,
        MultiBankCompositionLimits::default(),
    )
}

/// Compose diagnostic snapshots within an explicit all-in-memory resource
/// envelope. Every output snapshot contains an exact bank-indexed projection
/// of the source fact database, including global and cross-bank evidence.
pub fn compose_materialized_banks_v1_with_limits(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    limits: MultiBankCompositionLimits,
) -> Result<Vec<ProgramSnapshotV1>, SnapshotError> {
    Ok(
        compose_materialized_banks_validated_v2_with_limits(rom, base_facts, inputs, limits)?
            .into_diagnostic_snapshots(),
    )
}

/// Compose several banks and retain opaque execution authority for the exact
/// byte-verified V2 results. Cross-bank direct and exhaustive resolved-call
/// authority is derived inside this constructor; serialized `Fact` or block
/// reports cannot manufacture this wrapper.
pub fn compose_materialized_banks_validated_v2(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    compose_materialized_banks_validated_v2_with_limits(
        rom,
        base_facts,
        inputs,
        MultiBankCompositionLimits::default(),
    )
}

/// Compose authoritative snapshots within an explicit all-in-memory resource
/// envelope. Limits are checked before retaining any bank-local fact database;
/// the cross-bank record limit is enforced as unique authority is derived.
pub fn compose_materialized_banks_validated_v2_with_limits(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    limits: MultiBankCompositionLimits,
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    compose_materialized_banks_catalog_bound_with_limits(rom, base_facts, inputs, &[], limits)
}

/// Compose with move-only, catalog-bound authority for exact transfers whose
/// target VA is covered by multiple prepared generations. A capability is
/// consumed only when its ROM and complete `(source bank, site, kind, target)`
/// identity match an authority-reached edge. Calls confer callable authority;
/// jumps confer reachability only.
pub fn compose_materialized_banks_catalog_bound_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog_definition_sha256: [u8; 32],
    capabilities: &[CatalogBoundExactTransferV1],
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    let dense_pack_sha256 = dense_aot_pack_sha256_v1(dense_pack);
    if let Some(index) = capabilities.iter().position(|capability| {
        !capability.matches_composition_identity(
            &rom.sha256,
            dense_pack_sha256,
            topology,
            catalog_definition_sha256,
        )
    }) {
        return Err(SnapshotError::CatalogCapabilityIdentityMismatch { index });
    }
    compose_materialized_banks_catalog_bound_with_limits(
        rom,
        base_facts,
        inputs,
        capabilities,
        MultiBankCompositionLimits::default(),
    )
}

pub(super) fn compose_materialized_banks_catalog_bound_with_limits(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    capabilities: &[CatalogBoundExactTransferV1],
    limits: MultiBankCompositionLimits,
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    validate_unique_bank_names(inputs)?;
    let projection_index = check_multi_bank_limits(base_facts, inputs, limits)?;
    let mut prepared: Vec<PreparedBank> = inputs
        .iter()
        .map(|input| {
            let projected_facts = projection_index.project(input.bank);
            prepare_materialized_bank(
                rom,
                &projected_facts,
                MaterializedBankInput {
                    bank: input.bank,
                    va_start: input.va_start,
                    bytes: input.bytes,
                    seed_roots: input.seed_roots,
                },
                limits.materialized_image,
            )
        })
        .collect::<Result<_, _>>()?;
    let interval_index = BankIntervalIndex::new(&prepared);

    // Collect every proven-source direct call whose target lands inside some
    // OTHER prepared bank's proven VA range, keyed by that target bank. The
    // source bank's own closure has already recorded its in-bank calls; here we
    // look only across bank boundaries, which no single-bank composition sees.
    let mut cross_calls: BTreeMap<String, BTreeSet<CrossBankAuthoritativeCall>> = BTreeMap::new();
    // A target covered by exactly one bank identifies both the bytes and the
    // generation unambiguously. Physical and proven VROM-backed banks are
    // equally byte-verifiable here. A VA covered by several generations does
    // not identify executable bytes, so it confers neither reachability nor
    // semantic authority until a typed activation-compatibility capability
    // selects the generation.
    let mut cross_call_count = 0;
    loop {
        let mut newly_reachable = BTreeMap::<String, BTreeSet<u32>>::new();
        let mut newly_semantic = BTreeMap::<String, BTreeSet<u32>>::new();
        for source in &prepared {
            let mut authority_transfers = authority_reachable_direct_calls(source)
                .into_iter()
                .map(|(source_pc, target_pc)| {
                    (
                        source_pc,
                        target_pc,
                        ExactTransferKindV1::Call,
                        Some(CrossBankAuthoritativeCallKind::Direct),
                    )
                })
                .collect::<Vec<_>>();
            authority_transfers.extend(authority_reachable_direct_jumps(source).into_iter().map(
                |(source_pc, target_pc)| (source_pc, target_pc, ExactTransferKindV1::Jump, None),
            ));
            for block in &source.authority_closure.cfg.blocks {
                let crate::cfg::BlockTerminator::ResolvedIndirect {
                    targets,
                    via_call: true,
                } = &block.terminator
                else {
                    continue;
                };
                let Some(source_pc) = authoritative_resolved_call_site(source, block, targets)
                else {
                    continue;
                };
                authority_transfers.extend(targets.iter().copied().map(|target_pc| {
                    (
                        source_pc,
                        target_pc,
                        ExactTransferKindV1::Call,
                        Some(CrossBankAuthoritativeCallKind::ExhaustiveResolved),
                    )
                }));
            }
            authority_transfers.sort_unstable();
            authority_transfers.dedup();
            for (source_pc, target_pc, transfer_kind, call_kind) in authority_transfers {
                if !target_pc.is_multiple_of(4)
                    || (source.va_start <= target_pc && target_pc < source.va_end)
                {
                    continue;
                }
                let target_indices =
                    interval_index.matching_other_banks(source.bank.as_str(), target_pc);
                let target_index = match target_indices.as_slice() {
                    [target_index] => Some(*target_index),
                    [] => None,
                    _ => capabilities.iter().find_map(|capability| {
                        let (cap_source_bank, cap_source_pc, cap_kind, cap_target_pc) =
                            capability.exact_edge();
                        (capability.normalized_rom_sha256() == rom.sha256
                            && cap_source_bank == source.bank
                            && cap_source_pc == source_pc
                            && cap_kind == transfer_kind
                            && cap_target_pc == target_pc)
                            .then(|| capability.selected_target().0)
                            .and_then(|target_bank| {
                                target_indices
                                    .iter()
                                    .copied()
                                    .find(|index| prepared[*index].bank == target_bank)
                            })
                    }),
                };
                let Some(target_index) = target_index else {
                    continue;
                };
                let target_bank = prepared[target_index].bank.as_str();
                if call_kind.is_some()
                    && !prepared[target_index]
                        .semantic_cross_bank_roots
                        .contains(&target_pc)
                {
                    newly_semantic
                        .entry(target_bank.to_owned())
                        .or_default()
                        .insert(target_pc);
                }
                if !prepared[target_index]
                    .cross_bank_reachability_roots
                    .contains(&target_pc)
                {
                    newly_reachable
                        .entry(prepared[target_index].bank.clone())
                        .or_default()
                        .insert(target_pc);
                }
                if let Some(kind) = call_kind {
                    insert_cross_bank_authority(
                        &mut cross_calls,
                        prepared[target_index].bank.as_str(),
                        CrossBankAuthoritativeCall {
                            source_bank: source.bank.clone(),
                            source_pc,
                            target_pc,
                            kind,
                        },
                        &mut cross_call_count,
                        limits.max_cross_bank_authority_records,
                    )?;
                }
            }
        }

        if newly_reachable.is_empty() && newly_semantic.is_empty() {
            break;
        }
        let changed_banks: BTreeSet<String> = newly_reachable
            .keys()
            .chain(newly_semantic.keys())
            .cloned()
            .collect();
        for bank in &mut prepared {
            if changed_banks.contains(&bank.bank) {
                let empty = BTreeSet::new();
                let projected_facts = projection_index.project(&bank.bank);
                expand_prepared_cross_bank_authority(
                    bank,
                    &projected_facts,
                    newly_reachable.get(&bank.bank).unwrap_or(&empty),
                    newly_semantic.get(&bank.bank).unwrap_or(&empty),
                )?;
            }
        }
    }

    // Traversal hints are diagnostic coverage only. Delay their potentially
    // large CFGs until direct and exhaustive-resolved callable authority reach
    // one monotone fixed point, then build each broad closure once.
    for bank in &mut prepared {
        let projected_facts = projection_index.project(&bank.bank);
        refresh_prepared_traversal_closure(bank, &projected_facts)?;
    }

    let mut snapshots = Vec::with_capacity(prepared.len());
    for mut bank in prepared {
        let calls = cross_calls.remove(&bank.bank).unwrap_or_default();
        // Record the real cross-bank edge as a fact and promote its target to an
        // external authorized root. The fact makes the incoming edge visible to
        // owner proof (a cross-bank call into an interior is still an ambiguity
        // blocker; only a call to the exact entry confers authority); the root
        // set is what discharges `EntryNotAuthoritative` for that entry.
        //
        // Deliberately NOT re-seeded into the CFG closure: the target bank's
        // own traversal already reaches this code, and injecting hundreds of
        // extra partition roots fractures the partition into ambiguity (measured
        // to erase even the in-bank owners). Authority alone is the sound,
        // additive change — a same-bank direct call's authority extended across
        // the boundary, nothing weaker and nothing that re-shapes the partition.
        let mut external_roots = bank.authorized_callable_roots.clone();
        for call in &calls {
            let source = BankAddr::new(call.source_bank.as_str(), call.source_pc);
            let target = BankAddr::new(bank.bank.as_str(), call.target_pc);
            insert_unique(
                &mut bank.facts,
                match call.kind {
                    CrossBankAuthoritativeCallKind::Direct => Fact::DirectCall { source, target },
                    CrossBankAuthoritativeCallKind::ExhaustiveResolved => {
                        Fact::ResolvedCall { source, target }
                    }
                },
            );
            external_roots.insert(call.target_pc);
            bank.cross_bank_reachability_roots.insert(call.target_pc);
        }
        // Vetted cross-bank roots must enter the authority closure so block
        // reachability and both owner passes share the same authority. Keep
        // them out of the already-built broad closure: re-partitioning that
        // geometry was measured to fracture owners into ambiguity.
        let authority_closure = build_cross_bank_authority_closure(
            base_facts,
            bank.bank.as_str(),
            &bank.bytes,
            bank.va_start,
            &bank.authorized_callable_roots,
            &bank.cross_bank_reachability_roots,
        );
        bank.authority_closure = authority_closure;
        validate_authoritative_delay_slot_roots(&bank)?;
        snapshots.push(finish_materialized_bank(rom, bank, &external_roots)?);
    }
    Ok(ValidatedComposedSnapshotsV2 { snapshots })
}

pub(super) fn authoritative_resolved_call_site(
    source: &PreparedBank,
    block: &crate::cfg::BasicBlock,
    cfg_targets: &[u32],
) -> Option<u32> {
    let crate::cfg::BlockTerminator::ResolvedIndirect { targets, .. } = &block.terminator else {
        return None;
    };
    (targets == cfg_targets)
        .then(|| exhaustive_authority_call_site(&source.authority_closure, block))
        .flatten()
}
