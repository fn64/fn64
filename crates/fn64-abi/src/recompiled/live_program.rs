use super::*;

/// Whether the continuous executable-memory snapshot runs at every dispatch.
///
/// The snapshot re-reads and re-compares the whole watched region -- 1 MiB on
/// WM2000 -- at every dispatch boundary, and boundaries are per block
/// transfer, per host-ABI call and per thread selection. It measured as ~95%
/// of execution time, which is most of why the certified lane runs 23.5x
/// slower than realtime.
///
/// What it checks is that executable bytes did not change without a writer
/// declaring the change. That is a property WRITE ATTRIBUTION already
/// provides: every write path routes through
/// `record_executable_and_renderer_write` and lands in one of eight fixed
/// `WriterChannel`s. Measured over WM2000's full route -- 505,140 journal
/// entries -- ZERO changed ranges lacked a covering declaration.
///
/// So the snapshot guards against fn64 failing to attribute its own writes,
/// not against anything the guest does. That is worth asserting continuously
/// in gates and CI, and not worth paying for in a lane meant to be played.
///
/// Default ON. `FN64_FAST_MUTATION_JOURNAL=1` turns it off, and the
/// per-generation digest at activation plus write attribution both keep
/// running either way -- those are what receipts assert.
///
/// Latched once: a run must not change verification strength midway.
fn continuous_snapshot_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !std::env::var_os("FN64_FAST_MUTATION_JOURNAL").is_some_and(|value| value == "1")
    })
}

impl CanonicalExecutableMutationStateV1 {
    pub(super) fn new(ranges: &[(u32, u32)]) -> Self {
        assert!(
            !ranges.is_empty(),
            "canonical mutation state requires executable backing"
        );
        let mut watched = Vec::with_capacity(ranges.len());
        let mut previous_end = 0;
        for &(physical_start, physical_end) in ranges {
            assert!(
                physical_start < physical_end
                    && physical_end <= fn64_recomp_rs::RDRAM_LEN as u32
                    && (watched.is_empty() || physical_start > previous_end),
                "canonical executable mutation range is invalid or non-canonical: [{physical_start:#010x}, {physical_end:#010x})"
            );
            watched.push(WatchedExecutableBytesV1 {
                physical_start,
                physical_end,
                expected: Vec::new(),
                expected_storage_order: Vec::new(),
            });
            previous_end = physical_end;
        }
        Self {
            watched,
            sealed: false,
            expected_sha256: None,
            entries: Vec::new(),
            journal_root_sha256: [0; 32],
            next_sequence: 0,
            next_transaction_id: 0,
            host_transactions: BTreeMap::new(),
            host_abi_writer_trace: None,
            next_child_transaction_id: 0,
            active_child_transaction: None,
            poison: None,
        }
    }

    pub(super) fn assert_not_poisoned(&self) {
        if let Some(reason) = &self.poison {
            recompiled_gap_panic(format!(
                "canonical executable mutation owner is poisoned: {reason}"
            ));
        }
    }

    pub(super) fn poison(&mut self, reason: String) {
        if self.poison.is_none() {
            self.poison = Some(reason);
        }
    }

    pub(super) fn begin_child_transaction(&mut self) -> u64 {
        self.assert_not_poisoned();
        assert!(
            self.active_child_transaction.is_none(),
            "canonical executable mutation owner already has an active child writer transaction"
        );
        let id = self.next_child_transaction_id;
        self.next_child_transaction_id = self
            .next_child_transaction_id
            .checked_add(1)
            .expect("canonical child writer transaction id overflow");
        self.active_child_transaction = Some(id);
        id
    }

    pub(super) fn assert_active_child_transaction(&self, id: u64) {
        self.assert_not_poisoned();
        assert_eq!(
            self.active_child_transaction,
            Some(id),
            "canonical child writer transaction {id} is not the active owner"
        );
    }

    pub(super) fn finish_child_transaction(&mut self, id: u64) {
        self.assert_active_child_transaction(id);
        self.active_child_transaction = None;
    }

    pub(super) fn begin_host_transaction(
        &mut self,
        thread: ThreadId,
        target: GuestPc,
        resume: ExecutionKey,
    ) -> HostMutationTransactionTokenV1 {
        self.assert_not_poisoned();
        let transaction_id = self.next_transaction_id;
        self.next_transaction_id = self
            .next_transaction_id
            .checked_add(1)
            .expect("canonical host mutation transaction id overflow");
        let frame = OpenHostMutationTransactionEvidenceV1 {
            transaction_id,
            thread,
            target,
            resume,
        };
        self.host_transactions
            .entry(thread)
            .or_default()
            .push(frame);
        if let Some(trace) = &mut self.host_abi_writer_trace {
            trace.events.push(HostAbiWriterTraceEventV1::Started(frame));
        }
        HostMutationTransactionTokenV1 {
            transaction_id,
            thread,
        }
    }

    pub(super) fn active_host_transaction(&self, thread: ThreadId) -> Option<HostMutationTransactionTokenV1> {
        self.host_transactions
            .get(&thread)
            .and_then(|stack| stack.last())
            .map(|frame| HostMutationTransactionTokenV1 {
                transaction_id: frame.transaction_id,
                thread,
            })
    }

    fn assert_active_host_transaction(&self, token: HostMutationTransactionTokenV1) {
        self.assert_not_poisoned();
        let actual = self
            .active_host_transaction(token.thread)
            .unwrap_or_else(|| {
                recompiled_gap_panic(format!(
                    "host mutation transaction {} for thread {} is not active",
                    token.transaction_id, token.thread
                ))
            });
        if actual != token {
            recompiled_gap_panic(format!(
                "host mutation transaction stack mismatch for thread {}: expected top {}, received {}",
                token.thread, actual.transaction_id, token.transaction_id
            ));
        }
    }

    pub(super) fn finish_host_transaction(&mut self, token: HostMutationTransactionTokenV1) {
        self.assert_active_host_transaction(token);
        let stack = self
            .host_transactions
            .get_mut(&token.thread)
            .expect("active host transaction stack disappeared");
        let frame = stack.pop().expect("active host transaction stack is empty");
        assert_eq!(frame.transaction_id, token.transaction_id);
        if stack.is_empty() {
            self.host_transactions.remove(&token.thread);
        }
        if let Some(trace) = &mut self.host_abi_writer_trace {
            trace.events.push(HostAbiWriterTraceEventV1::Finished {
                transaction_id: token.transaction_id,
                thread: token.thread,
            });
        }
    }

    pub(super) fn record_host_abi_boundary(
        &mut self,
        token: HostMutationTransactionTokenV1,
        first_new_entry: usize,
    ) {
        self.assert_active_host_transaction(token);
        let journal_sequences = self.entries[first_new_entry..]
            .iter()
            .map(|entry| {
                assert!(
                    entry
                        .declared_writes
                        .iter()
                        .all(|declaration| declaration.channel == WriterChannel::HostAbi),
                    "Host ABI ordering boundary committed a non-HostAbi declaration"
                );
                entry.sequence
            })
            .collect();
        if let Some(trace) = &mut self.host_abi_writer_trace {
            trace.events.push(HostAbiWriterTraceEventV1::Boundary {
                transaction_id: token.transaction_id,
                thread: token.thread,
                journal_sequences,
            });
        }
    }

    pub(super) fn from_bootstrap(evidence: &BootstrapOrImportValidationEvidenceV1, storage: &[u8]) -> Self {
        let ranges = evidence
            .watched_ranges
            .iter()
            .map(|range| (range.physical_start, range.physical_end))
            .collect::<Vec<_>>();
        assert_eq!(
            watched_bytes_sha256(storage, &ranges),
            evidence.watched_sha256,
            "validated bootstrap watched bytes changed before journal initialization"
        );
        let mut state = Self::new(&ranges);
        state.seal_with(|_| 0);
        let view = fn64_runtime::RdramView::from_storage(storage);
        let snapshot = state
            .read_snapshot_from_view(&view);
        let events = evidence
            .publications
            .iter()
            .map(|publication| GuestWriteEvent::Range {
                channel: WriterChannel::BootstrapOrImport,
                physical_offset: publication.physical_start,
                len: publication.physical_end - publication.physical_start,
            })
            .collect();
        state.commit_snapshot(snapshot, events, Vec::new());
        state
    }

    pub(super) fn required_physical_end(&self) -> u32 {
        self.watched
            .last()
            .expect("canonical mutation state has no watched ranges")
            .physical_end
    }

    /// The physical ranges this state watches, as plain tuples.
    ///
    /// Exposed so the renderer/RSP mutation tracker can snapshot exactly what
    /// `commit_snapshot` will later compare against. Watching a different set
    /// there is what let an undeclared executable write slip through.
    /// The sealed baseline byte for one physical address, if watched.
    ///
    /// Diagnostic for the zero-baseline blocker: it answers whether `expected`
    /// ever received the published ROM bytes, which the digests cannot say.
    pub(super) fn expected_byte_at(&self, physical: u32) -> Option<u8> {
        self.watched.iter().find_map(|range| {
            (range.physical_start <= physical && physical < range.physical_end)
                .then(|| {
                    range
                        .expected
                        .get((physical - range.physical_start) as usize)
                        .copied()
                })
                .flatten()
        })
    }

    pub(super) fn watched_ranges(&self) -> Vec<(u32, u32)> {
        self.watched
            .iter()
            .map(|range| (range.physical_start, range.physical_end))
            .collect()
    }

    /// Snapshot the watched ranges straight from an RDRAM view.
    ///
    /// [`Self::read_snapshot`] takes a per-byte closure, which forces a
    /// bounds-checked call and a lane XOR for every byte. On WM2000 the
    /// watched region is the 1 MiB boot bank and this runs at every dispatch
    /// boundary, so that cost was measured at 21.6 ms per executor step --
    /// about 36 steps/second, dominating every run.
    ///
    /// `copy_logical_bytes` does the same work one native word at a time, so
    /// the check is amortized over four bytes. Callers that genuinely have
    /// only a byte reader keep using `read_snapshot`; the hot paths should
    /// use this.
    pub(super) fn read_snapshot_from_view(
        &self,
        view: &fn64_runtime::RdramView<'_>,
    ) -> Vec<Vec<u8>> {
        self.watched
            .iter()
            .map(|range| {
                let mut bytes = vec![0u8; (range.physical_end - range.physical_start) as usize];
                view.copy_logical_bytes(
                    fn64_runtime::RdramAddr::from_offset(range.physical_start),
                    &mut bytes,
                );
                bytes
            })
            .collect()
    }

    /// Whether every watched byte still equals the sealed baseline.
    ///
    /// The same question `current_changed_ranges(&read_snapshot_from_view(v))
    /// .is_empty()` answers, decided without building the snapshot. On WM2000
    /// the watched region is the 1 MiB boot bank and "nothing changed" is the
    /// overwhelmingly common answer, so the snapshot the old form allocated,
    /// copied, and word-reversed was thrown away unread on nearly every
    /// dispatch. This settles it with one `memcmp` per range.
    ///
    /// A `false` answer commits to nothing: callers fall back to the copying
    /// path, so every diagnostic, panic message and journal entry is produced
    /// by exactly the code that produced it before.
    pub(super) fn matches_view(&self, view: &fn64_runtime::RdramView<'_>) -> bool {
        self.sealed && self.watched.iter().all(|range| range.matches_storage(view))
    }

    pub(super) fn read_snapshot(&self, mut read_physical_byte: impl FnMut(u32) -> u8) -> Vec<Vec<u8>> {
        self.watched
            .iter()
            .map(|range| {
                (range.physical_start..range.physical_end)
                    .map(&mut read_physical_byte)
                    .collect()
            })
            .collect()
    }

    pub(super) fn digest_snapshot(&self, snapshot: &[Vec<u8>]) -> [u8; 32] {
        let mut digest = sha2::Sha256::new();
        for (range, bytes) in self.watched.iter().zip(snapshot) {
            digest.update(range.physical_start.to_be_bytes());
            digest.update(range.physical_end.to_be_bytes());
            digest.update(bytes);
        }
        digest.finalize().into()
    }

    pub(super) fn seal_with(&mut self, read_physical_byte: impl FnMut(u32) -> u8) {
        if self.sealed {
            return;
        }
        let snapshot = self.read_snapshot(read_physical_byte);
        let expected_sha256 = self.digest_snapshot(&snapshot);
        for (range, bytes) in self.watched.iter_mut().zip(snapshot) {
            range.set_expected(bytes);
        }
        self.journal_root_sha256 = canonical_mutation_initial_root(
            expected_sha256,
            self.watched
                .iter()
                .map(|range| PendingExecutableWriteEvidenceSnapshot {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                }),
        );
        self.expected_sha256 = Some(expected_sha256);
        self.sealed = true;
    }

    pub(super) fn current_changed_ranges(&self, snapshot: &[Vec<u8>]) -> Vec<(u32, u32)> {
        let mut changed = Vec::new();
        for (range, current) in self.watched.iter().zip(snapshot) {
            assert_eq!(range.expected.len(), current.len());
            let expected = range.expected.as_slice();
            // Byte-at-a-time over the 1 MiB boot bank, at every dispatch
            // boundary, with "nothing changed" as the overwhelmingly common
            // answer. `==` on slices lowers to memcmp, so settle that case in
            // one shot and only walk bytes once something actually differs.
            if expected == current.as_slice() {
                continue;
            }
            let mut index = 0;
            while index < current.len() {
                // Skip equal bytes a chunk at a time. Chunk boundaries do not
                // affect the result: the byte loop below still finds the exact
                // first and last differing byte, so the emitted ranges are
                // identical to the scalar scan's.
                const CHUNK: usize = 16;
                while index + CHUNK <= current.len()
                    && expected[index..index + CHUNK] == current[index..index + CHUNK]
                {
                    index += CHUNK;
                }
                if index >= current.len() {
                    break;
                }
                if expected[index] == current[index] {
                    index += 1;
                    continue;
                }
                let start = index;
                index += 1;
                while index < current.len() && expected[index] != current[index] {
                    index += 1;
                }
                changed.push((
                    range.physical_start + start as u32,
                    range.physical_start + index as u32,
                ));
            }
        }
        changed
    }

    pub(super) fn clipped_declarations(
        &self,
        events: &[GuestWriteEvent],
    ) -> Vec<AttributedExecutableWriteEvidenceV1> {
        let mut declarations = Vec::new();
        for &event in events {
            let (physical_start, byte_len) = event.range();
            let physical_end = physical_start.checked_add(byte_len).unwrap_or_else(|| {
                recompiled_gap_panic(format!(
                    "attributed executable write overflows: {physical_start:#010x} + {byte_len:#x}"
                ))
            });
            for watched in &self.watched {
                let start = physical_start.max(watched.physical_start);
                let end = physical_end.min(watched.physical_end);
                if start < end {
                    declarations.push(AttributedExecutableWriteEvidenceV1 {
                        channel: event.channel(),
                        physical_start: start,
                        physical_end: end,
                    });
                }
            }
        }
        declarations
    }

    fn first_uncovered_changed_range(
        declarations: &[AttributedExecutableWriteEvidenceV1],
        changed: &[(u32, u32)],
    ) -> Option<(u32, u32)> {
        let mut intervals = declarations
            .iter()
            .map(|declaration| (declaration.physical_start, declaration.physical_end))
            .collect::<Vec<_>>();
        intervals.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(intervals.len());
        for (start, end) in intervals {
            if let Some((_, previous_end)) = merged.last_mut() {
                if start <= *previous_end {
                    *previous_end = (*previous_end).max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }

        let mut interval_index = 0;
        for &(changed_start, changed_end) in changed {
            while interval_index < merged.len() && merged[interval_index].1 <= changed_start {
                interval_index += 1;
            }
            let mut cursor = changed_start;
            let mut candidate = interval_index;
            while candidate < merged.len() && merged[candidate].0 <= cursor {
                cursor = cursor.max(merged[candidate].1);
                if cursor >= changed_end {
                    break;
                }
                candidate += 1;
            }
            if cursor < changed_end {
                return Some((cursor, changed_end));
            }
        }
        None
    }

    /// Run the dispatch reconcile without building a snapshot, if bytes match.
    ///
    /// Returns whether it fully discharged the reconcile. The guards are the
    /// same three [`Self::reconcile_snapshot_before_dispatch`] runs, in the
    /// same order, and they run unconditionally here -- only the comparison is
    /// avoided, and only when [`Self::matches_view`] has already proved it
    /// would find nothing. `false` means "not decided", and the caller then
    /// runs the copying path unchanged, so any change is reported by exactly
    /// the original code with its original diagnostics.
    pub(super) fn reconcile_matched_before_dispatch(
        &self,
        view: &fn64_runtime::RdramView<'_>,
    ) -> bool {
        self.assert_not_poisoned();
        assert!(
            self.sealed,
            "canonical executable mutation state is not sealed"
        );
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|writes| writes.borrow().len());
        assert_eq!(
            pending, 0,
            "canonical executable dispatch attempted with {pending} attributed write(s) not yet invalidated"
        );
        self.matches_view(view)
    }

    pub(super) fn reconcile_snapshot_before_dispatch(&mut self, snapshot: Vec<Vec<u8>>) {
        self.assert_not_poisoned();
        assert!(
            self.sealed,
            "canonical executable mutation state is not sealed"
        );
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|writes| writes.borrow().len());
        assert_eq!(
            pending, 0,
            "canonical executable dispatch attempted with {pending} attributed write(s) not yet invalidated"
        );
        if let Some((physical_start, physical_end)) =
            self.current_changed_ranges(&snapshot).into_iter().next()
        {
            // Report what the journal knows about this byte. The panic site in
            // `execution.rs` cannot: `mutation_evidence_snapshot()` returns
            // None there, so its dump prints nothing. Here the state is in
            // hand, and the question is precisely whether some earlier entry
            // DID declare this address -- which separates "no writer
            // attributed it" from "a writer attributed it and the baseline was
            // not advanced", the two causes that produce this same message.
            // The actual VALUES. Every writer-side hypothesis is eliminated, so
            // what matters now is whether `expected` was ever correct for this
            // byte -- which the digests cannot say and no run has captured.
            let (expected_byte, live_byte) = self
                .watched
                .iter()
                .zip(&snapshot)
                .find_map(|(range, bytes)| {
                    (range.physical_start <= physical_start
                        && physical_start < range.physical_end)
                        .then(|| {
                            let index = (physical_start - range.physical_start) as usize;
                            (
                                range.expected.get(index).copied(),
                                bytes.get(index).copied(),
                            )
                        })
                })
                .unwrap_or((None, None));
            // A RANGE around the byte, so the shape of the discrepancy is
            // visible: a contiguous zero run means seal preceded publication
            // for that region, while scattered differences mean the
            // publication wrote something other than the ROM slice.
            let window = self
                .watched
                .iter()
                .zip(&snapshot)
                .find_map(|(range, bytes)| {
                    (range.physical_start <= physical_start
                        && physical_start < range.physical_end)
                        .then(|| {
                            let index = (physical_start - range.physical_start) as usize;
                            let lo = index.saturating_sub(8);
                            let hi = (index + 8).min(bytes.len());
                            let expected: Vec<String> = range.expected[lo..hi]
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect();
                            let live: Vec<String> =
                                bytes[lo..hi].iter().map(|byte| format!("{byte:02x}")).collect();
                            format!(
                                "at {:#010x} expected[{}] live[{}]",
                                range.physical_start + lo as u32,
                                expected.join(" "),
                                live.join(" "),
                            )
                        })
                })
                .unwrap_or_default();
            let declarations = self
                .entries
                .iter()
                .flat_map(|entry| {
                    entry.declared_writes.iter().map(move |write| {
                        (entry.sequence, write.channel, write.physical_start, write.physical_end)
                    })
                })
                .filter(|(_, _, start, end)| *start <= physical_start && physical_end <= *end)
                .map(|(sequence, channel, start, end)| {
                    format!("seq={sequence} {channel:?} [{start:#010x},{end:#010x})")
                })
                .collect::<Vec<_>>();
            recompiled_gap_panic(format!(
                "unjournaled executable mutation changed physical RDRAM [{physical_start:#010x}, {physical_end:#010x}) before canonical static dispatch; \
                 expected={expected_byte:?} live={live_byte:?} window={window} \
                 journal_entries={} covering_declarations={} [{}]",
                self.entries.len(),
                declarations.len(),
                declarations.join("; "),
            ));
        }
    }

    /// Accept the current bytes as the baseline without journalling a batch.
    ///
    /// Used when a second commit path finds the queue already drained: there is
    /// nothing to attribute, but leaving `expected` stale makes the next
    /// dispatch re-detect a change that was already accounted for.
    pub(super) fn adopt_snapshot(&mut self, snapshot: Vec<Vec<u8>>) {
        self.assert_not_poisoned();
        if !self.sealed {
            return;
        }
        // The overwhelmingly common case is that nothing changed: this path runs
        // on every dispatch whose declaration queue was already drained. Equal
        // bytes have an equal digest by definition, so re-hashing every watched
        // byte to re-derive a value we already hold is pure cost -- and it was
        // ~30% of the shell's entire profile before this check.
        if self
            .watched
            .iter()
            .map(|range| &range.expected)
            .eq(snapshot.iter())
        {
            return;
        }
        self.expected_sha256 = Some(self.digest_snapshot(&snapshot));
        for (range, bytes) in self.watched.iter_mut().zip(snapshot) {
            range.set_expected(bytes);
        }
    }

    pub(super) fn commit_snapshot(
        &mut self,
        snapshot: Vec<Vec<u8>>,
        events: Vec<GuestWriteEvent>,
        mut invalidated_generations: Vec<GenerationId>,
    ) {
        self.assert_not_poisoned();
        assert!(
            self.sealed,
            "canonical executable mutation state is not sealed"
        );
        let changed = self.current_changed_ranges(&snapshot);
        let declarations = self.clipped_declarations(&events);
        if let Some((physical_start, physical_end)) =
            Self::first_uncovered_changed_range(&declarations, &changed)
        {
            // Name the writers that DID declare, and every range that changed.
            //
            // Without this the message says only that some byte was
            // undeclared, which is the one fact that does not narrow anything:
            // the whole question is which writer under-declared, and answering
            // it previously meant re-instrumenting and re-running a
            // multi-hour route. The channels and ranges are already in hand
            // here, so carrying them costs nothing on the failure path and
            // makes each occurrence self-explaining.
            let declared = declarations
                .iter()
                .map(|declaration| {
                    format!(
                        "{:?}[{:#010x},{:#010x})",
                        declaration.channel, declaration.physical_start, declaration.physical_end
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            let changed_ranges = changed
                .iter()
                .map(|(start, end)| format!("[{start:#010x},{end:#010x})"))
                .collect::<Vec<_>>()
                .join(" ");
            recompiled_gap_panic(format!(
                "executable mutation changed physical RDRAM [{physical_start:#010x}, {physical_end:#010x}) \
                 outside every attributed writer declaration; \
                 declared={{{declared}}} changed={{{changed_ranges}}} \
                 events={} declarations={}",
                events.len(),
                declarations.len()
            ));
        }
        if declarations.is_empty() && changed.is_empty() {
            return;
        }

        let before_sha256 = self
            .expected_sha256
            .expect("sealed mutation state has no expected digest");
        // With no changed range, the snapshot equals `expected` byte for byte,
        // so its digest IS `before_sha256` -- hashing 1 MiB to rediscover a
        // value already in hand is pure cost. This case is common: a writer
        // declares a store that writes back the same bytes, so the declaration
        // list is non-empty while nothing actually differs.
        //
        // Not an approximation. `changed` comes from `current_changed_ranges`,
        // which compares the snapshot against `expected` byte for byte, so
        // empty means equal and equal bytes have an equal digest.
        let after_sha256 = if changed.is_empty() {
            before_sha256
        } else {
            self.digest_snapshot(&snapshot)
        };
        invalidated_generations.sort_unstable();
        invalidated_generations.dedup();
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("canonical executable mutation sequence overflow");
        let mut entry = ExecutableMutationBatchEvidenceV1 {
            sequence,
            declared_writes: declarations,
            changed_ranges: changed
                .into_iter()
                .map(
                    |(physical_start, physical_end)| PendingExecutableWriteEvidenceSnapshot {
                        physical_start,
                        physical_end,
                    },
                )
                .collect(),
            before_sha256,
            after_sha256,
            invalidated_generations,
            journal_root_sha256: [0; 32],
        };
        entry.journal_root_sha256 = canonical_mutation_entry_root(self.journal_root_sha256, &entry);
        let journal_root_sha256 = entry.journal_root_sha256;
        self.entries.push(entry);
        for (range, bytes) in self.watched.iter_mut().zip(snapshot) {
            range.set_expected(bytes);
        }
        self.expected_sha256 = Some(after_sha256);
        self.journal_root_sha256 = journal_root_sha256;
    }

    pub(super) fn evidence_snapshot(&self) -> CanonicalExecutableMutationJournalEvidenceV1 {
        let open_host_transactions = self
            .host_transactions
            .values()
            .flat_map(|stack| stack.iter().copied())
            .collect();
        CanonicalExecutableMutationJournalEvidenceV1 {
            schema: CANONICAL_EXECUTABLE_MUTATION_JOURNAL_SCHEMA_V1.to_string(),
            watched_ranges: self
                .watched
                .iter()
                .map(|range| PendingExecutableWriteEvidenceSnapshot {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            sealed: self.sealed,
            expected_sha256: self.expected_sha256,
            entries: self.entries.clone(),
            journal_root_sha256: self.journal_root_sha256,
            pending_attributed_writes: PENDING_ATTRIBUTED_EXECUTABLE_WRITES
                .with(|writes| writes.borrow().len()),
            open_host_transactions,
        }
    }
}

impl CanonicalLiveBlockProgramV1 {
    pub(super) fn charge_canonical_instructions(&self, instructions: u32) {
        assert!(
            instructions > 0,
            "canonical instruction charge must be nonzero"
        );
        let charged = self
            .canonical_charged_instructions
            .get()
            .checked_add(u64::from(instructions))
            .expect("canonical BlockProgram instruction count overflow");
        if let Some(limit) = self.canonical_instruction_limit.get() {
            assert!(
                charged <= limit,
                "canonical BlockProgram exceeded exact instruction limit {limit}: charged {charged}"
            );
        }
        self.canonical_charged_instructions.set(charged);
    }

    pub(super) fn next_dispatch_budget(&self) -> InstructionBudget {
        let configured = self.install.budget();
        let Some(limit) = self.canonical_instruction_limit.get() else {
            return configured;
        };
        let charged = self.canonical_charged_instructions.get();
        let remaining = limit.checked_sub(charged).unwrap_or_else(|| {
            recompiled_gap_panic(format!(
                "canonical instruction limit {limit} is behind charged work {charged}"
            ))
        });
        if remaining == 0 {
            recompiled_gap_panic(format!(
                "canonical exact checkpoint limit {limit} was already reached"
            ));
        }
        let remaining = u32::try_from(remaining).unwrap_or(u32::MAX);
        InstructionBudget::new(configured.get().min(remaining))
            .expect("canonical exact checkpoint budget was checked against the minimum")
    }

    pub(super) fn publish_checkpoint(
        &self,
        instructions: u32,
        exit: BlockExit,
        prepared_continuation: Option<CanonicalPreparedContinuationV1>,
        ctx: &RsContext,
    ) {
        let thread = crate::current_thread_id("canonical checkpoint publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::Exact(CanonicalThreadCheckpointEvidenceV1 {
                thread,
                cpu: ctx.evidence_snapshot_v1(),
                charged_instructions: instructions,
                canonical_charged_instructions_at_publication: self
                    .canonical_charged_instructions
                    .get(),
                pending_exit: exit,
                prepared_continuation,
            }),
        );
    }

    pub(super) fn publish_opaque_host(&self, target: GuestPc, resume: ExecutionKey) {
        let thread = crate::current_thread_id("canonical host publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::OpaqueHostInFlight {
                thread,
                target,
                resume,
            },
        );
    }

    pub(super) fn publish_parked_fault(&self, fault: CpuFault, ctx: &RsContext) {
        let thread = crate::current_thread_id("canonical parked-fault publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::ParkedFaultOpaque {
                thread,
                post_exception_cpu: ctx.evidence_snapshot_v1(),
                fault,
                canonical_charged_instructions_at_publication: self
                    .canonical_charged_instructions
                    .get(),
            },
        );
    }

    pub(super) fn publish_returned(&self, ctx: &RsContext) {
        let thread = crate::current_thread_id("canonical return publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::Returned {
                thread,
                cpu: ctx.evidence_snapshot_v1(),
            },
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    pub(super) fn enable_dynamic_mapped_execution(&self) {
        let mut dynamic = self.dynamic_units.borrow_mut();
        assert!(
            dynamic.is_none(),
            "dynamic mapped execution is already installed"
        );
        *dynamic = Some(fn64_recomp_rs::DynamicMappedUnitCatalogV1::new_linked());
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    pub(super) fn enable_dynamic_mapped_execution_with_exact_static_key_withheld(
        &self,
        selected: ExecutionKey,
    ) {
        assert_eq!(
            selected,
            self.install.entry(),
            "operational exact-key withholding must select the canonical catalog entry {} rather than {selected}",
            self.install.entry()
        );
        let resolved = self.resolve_transfer(selected.bank, selected.pc).unwrap_or_else(|fault| {
            recompiled_gap_panic(format!(
                "operational exact-key withholding selected {selected}, which is absent from the installed static catalog: {fault}"
            ))
        });
        assert_eq!(
            resolved, selected,
            "operational exact-key withholding selected {selected}, but the installed static catalog resolves that address as {resolved}"
        );
        self.enable_dynamic_mapped_execution();
        self.dynamic_withheld_static_key.set(Some(selected));
    }

    pub(super) fn dynamic_execution_installed(&self) -> bool {
        #[cfg(feature = "dynamic-mapped-runtime")]
        {
            self.dynamic_units.borrow().is_some()
        }
        #[cfg(not(feature = "dynamic-mapped-runtime"))]
        {
            false
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    pub(super) fn record_dynamic_execution(
        &self,
        attempted_entry: ExecutionKey,
        run: &fn64_recomp_rs::DynamicMappedRunV1,
    ) {
        let charged_instructions = u64::from(run.run.instructions);
        let unsupported_exit = matches!(
            run.run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnsupportedInstruction { .. },
                ..
            })
        );
        let mutation_sequence = self
            .mutation_state
            .as_ref()
            .and_then(|state| state.borrow().entries.last().map(|entry| entry.sequence));
        let mut aggregates = self.dynamic_execution_aggregates.borrow_mut();
        if !aggregates.contains_key(&run.identity)
            && aggregates.len() >= DYNAMIC_EXECUTION_AGGREGATE_CAPACITY
        {
            self.dynamic_dropped_identity_activations.set(
                self.dynamic_dropped_identity_activations
                    .get()
                    .checked_add(1)
                    .expect("dropped dynamic identity activation count overflow"),
            );
            self.dynamic_dropped_identity_charged_instructions.set(
                self.dynamic_dropped_identity_charged_instructions
                    .get()
                    .checked_add(charged_instructions)
                    .expect("dropped dynamic identity instruction count overflow"),
            );
            if unsupported_exit {
                self.dynamic_dropped_identity_unsupported_exits.set(
                    self.dynamic_dropped_identity_unsupported_exits
                        .get()
                        .checked_add(1)
                        .expect("dropped dynamic identity unsupported-exit count overflow"),
                );
            }
            return;
        }
        let aggregate =
            aggregates
                .entry(run.identity)
                .or_insert_with(|| DynamicMappedExecutionAggregateV1 {
                    identity: run.identity,
                    admitted_entry: run.entry,
                    instructions: run.instructions.clone(),
                    attempted_entries: Vec::new(),
                    activations: 0,
                    charged_instructions: 0,
                    unsupported_exits: 0,
                    first_mutation_sequence: mutation_sequence,
                    last_mutation_sequence: mutation_sequence,
                    last_exit: run.run.exit,
                });
        assert_eq!(
            aggregate.admitted_entry.bank, run.entry.bank,
            "dynamic identity changed its content-derived bank"
        );
        assert_eq!(
            aggregate.instructions, run.instructions,
            "dynamic identity changed its physical instruction set"
        );
        match aggregate
            .attempted_entries
            .binary_search_by_key(&attempted_entry, |entry| entry.attempted_entry)
        {
            Ok(index) => {
                let entry = &mut aggregate.attempted_entries[index];
                entry.activations = entry
                    .activations
                    .checked_add(1)
                    .expect("dynamic attempted-entry activation count overflow");
                entry.charged_instructions = entry
                    .charged_instructions
                    .checked_add(charged_instructions)
                    .expect("dynamic attempted-entry instruction count overflow");
                if unsupported_exit {
                    entry.unsupported_exits = entry
                        .unsupported_exits
                        .checked_add(1)
                        .expect("dynamic attempted-entry unsupported-exit count overflow");
                }
            }
            Err(index)
                if aggregate.attempted_entries.len()
                    < DYNAMIC_ATTEMPTED_ENTRIES_PER_AGGREGATE_CAPACITY =>
            {
                aggregate.attempted_entries.insert(
                    index,
                    DynamicMappedEntryCountV1 {
                        attempted_entry,
                        activations: 1,
                        charged_instructions,
                        unsupported_exits: u64::from(unsupported_exit),
                    },
                )
            }
            Err(_) => {
                self.dynamic_dropped_attempted_entry_activations.set(
                    self.dynamic_dropped_attempted_entry_activations
                        .get()
                        .checked_add(1)
                        .expect("dropped dynamic attempted-entry activation count overflow"),
                );
                self.dynamic_dropped_attempted_entry_charged_instructions
                    .set(
                        self.dynamic_dropped_attempted_entry_charged_instructions
                            .get()
                            .checked_add(charged_instructions)
                            .expect("dropped dynamic attempted-entry instruction count overflow"),
                    );
                if unsupported_exit {
                    self.dynamic_dropped_attempted_entry_unsupported_exits.set(
                        self.dynamic_dropped_attempted_entry_unsupported_exits
                            .get()
                            .checked_add(1)
                            .expect(
                                "dropped dynamic attempted-entry unsupported-exit count overflow",
                            ),
                    );
                }
            }
        }
        aggregate.activations = aggregate
            .activations
            .checked_add(1)
            .expect("dynamic activation count overflow");
        aggregate.charged_instructions = aggregate
            .charged_instructions
            .checked_add(charged_instructions)
            .expect("dynamic retired-instruction count overflow");
        if unsupported_exit {
            aggregate.unsupported_exits = aggregate
                .unsupported_exits
                .checked_add(1)
                .expect("dynamic unsupported-exit count overflow");
        }
        aggregate.last_mutation_sequence = mutation_sequence.or(aggregate.last_mutation_sequence);
        aggregate.last_exit = run.run.exit;
    }

    pub(super) fn mint_bootstrap_writer_completion(
        &self,
        storage: &[u8],
    ) -> Result<(), BootstrapWriterChannelCompletionErrorV1> {
        if self.dynamic_execution_installed() {
            return Err(BootstrapWriterChannelCompletionErrorV1::DynamicExecutionInstalled);
        }
        let bootstrap = self
            .bootstrap_evidence
            .as_ref()
            .expect("bootstrap writer completion requires bootstrap evidence");
        let state = self
            .mutation_state
            .as_ref()
            .expect("bootstrap writer completion requires mutation state")
            .borrow();
        let receipt = validate_bootstrap_writer_completion_state(
            self.writer_program_model_sha256,
            bootstrap,
            storage,
            &state,
        )?;
        let mut slot = self.bootstrap_writer_completion.borrow_mut();
        assert!(
            slot.is_none(),
            "bootstrap writer-channel completion authority was already minted"
        );
        *slot = Some(receipt);
        Ok(())
    }

    pub(super) fn begin_cpu_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<CpuWriterRuntimeTraceEpochV1>, CpuWriterRuntimeStateErrorV1> {
        if self.cpu_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(CpuWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(CpuWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(CpuWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(CpuWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(CpuWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_cpu_writer_quiescence(&state)?;
        drop(state);

        let epoch_id = next_cpu_writer_trace_epoch_id();
        self.cpu_writer_trace_epoch_id.set(Some(epoch_id));
        CPU_INSTRUCTION_STORE_TRACE.with(|trace| {
            *trace.borrow_mut() = Some(CpuInstructionStoreTraceV1 {
                epoch_id,
                events: Vec::new(),
            });
        });
        Ok(Some(CpuWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    pub(super) fn take_cpu_writer_runtime_state(
        &self,
        epoch: &CpuWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
    ) -> Result<Option<ValidatedCpuWriterRuntimeStateReceiptV1>, CpuWriterRuntimeStateErrorV1> {
        if self.cpu_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.cpu_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(CpuWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let trace = CPU_INSTRUCTION_STORE_TRACE.with(|trace| {
            let trace = trace.borrow();
            let trace = trace
                .as_ref()
                .ok_or(CpuWriterRuntimeStateErrorV1::TraceEpochNotArmed)?;
            if trace.epoch_id != epoch.epoch_id {
                return Err(CpuWriterRuntimeStateErrorV1::TraceEpochMismatch);
            }
            Ok(trace.events.clone())
        })?;
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(CpuWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_cpu_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            &trace,
        )?;
        CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
        self.cpu_writer_trace_epoch_id.set(None);
        self.cpu_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    pub(super) fn begin_host_abi_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<HostAbiWriterRuntimeTraceEpochV1>, HostAbiWriterRuntimeStateErrorV1> {
        if self.host_abi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(HostAbiWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(HostAbiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(HostAbiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(HostAbiWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let mut state = self
            .mutation_state
            .as_ref()
            .ok_or(HostAbiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow_mut();
        validate_host_abi_writer_quiescence(&state)?;
        let epoch_id = next_host_abi_writer_trace_epoch_id();
        state.host_abi_writer_trace = Some(HostAbiWriterTraceV1 {
            epoch_id,
            initial_journal_entry_count: u64::try_from(state.entries.len())
                .expect("Host ABI initial journal entry count exceeds u64"),
            events: Vec::new(),
        });
        Ok(Some(HostAbiWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    pub(super) fn take_host_abi_writer_runtime_state(
        &self,
        epoch: &HostAbiWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
    ) -> Result<Option<ValidatedHostAbiWriterRuntimeStateReceiptV1>, HostAbiWriterRuntimeStateErrorV1>
    {
        if self.host_abi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256 {
            return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(HostAbiWriterRuntimeStateErrorV1::Unsealed)?;
        let trace = state.borrow().host_abi_writer_trace.clone();
        if trace.as_ref().map(|trace| trace.epoch_id) != Some(epoch.epoch_id) {
            return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let abi_host_catalog = self
            .install
            .evidence()
            .abi_host_catalog
            .as_ref()
            .filter(|_| self.install.has_abi_host_catalog_authority());
        let receipt = validate_host_abi_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state.borrow(),
            trace.as_ref(),
        )?;
        state.borrow_mut().host_abi_writer_trace = None;
        self.host_abi_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    pub(super) fn begin_rsp_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<RspWriterRuntimeTraceEpochV1>, RspWriterRuntimeStateErrorV1> {
        if self.rsp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(RspWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(RspWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(RspWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(RspWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RspWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_rsp_writer_quiescence(&state)?;
        drop(state);

        let epoch_id = next_rsp_writer_trace_epoch_id();
        self.rsp_writer_trace_epoch_id.set(Some(epoch_id));
        crate::task_dispatch::begin_rsp_writer_trace_v1(epoch_id);
        Ok(Some(RspWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_rsp_writer_runtime_state(
        &self,
        epoch: &RspWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        pending_device_rsp_task: bool,
        pending_abi_rsp_work: bool,
    ) -> Result<Option<ValidatedRspWriterRuntimeStateReceiptV1>, RspWriterRuntimeStateErrorV1> {
        if self.rsp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.rsp_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(RspWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let trace = crate::task_dispatch::rsp_writer_trace_snapshot_v1(epoch.epoch_id)
            .ok_or(RspWriterRuntimeStateErrorV1::TraceEpochMismatch)?;
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RspWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_rsp_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            &trace,
            pending_device_rsp_task,
            pending_abi_rsp_work,
        )?;
        assert!(
            crate::task_dispatch::finish_rsp_writer_trace_v1(epoch.epoch_id),
            "validated RSP writer trace lost its exact epoch before consume"
        );
        self.rsp_writer_trace_epoch_id.set(None);
        self.rsp_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    pub(super) fn begin_rdp_renderer_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<RdpRendererWriterRuntimeTraceEpochV1>, RdpRendererWriterRuntimeStateErrorV1>
    {
        if self.rdp_renderer_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(RdpRendererWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RdpRendererWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_rdp_renderer_writer_quiescence(&state)?;
        let initial_journal_entry_count = u64::try_from(state.entries.len())
            .expect("RDP renderer initial journal entry count exceeds u64");
        let next_journal_entry_index = state.entries.len();
        drop(state);

        // Interleaving closed: OS thread A can retain a move-only epoch while
        // OS thread B installs an identical program model. A thread-local
        // counter could mint the same identity in both threads, allowing A's
        // token to consume B's trace arm; this process-global epoch cannot.
        let epoch_id = next_rdp_renderer_writer_trace_epoch_id();
        self.rdp_renderer_writer_trace_epoch_id.set(Some(epoch_id));
        RDP_RENDERER_WRITER_TRACE.with(|trace| {
            *trace.borrow_mut() = Some(RdpRendererWriterTraceV1 {
                epoch_id,
                program_model_sha256: self.writer_program_model_sha256,
                initial_journal_entry_count,
                next_journal_entry_index,
                publications: Vec::new(),
                rejected_journal_sequences: Vec::new(),
            });
        });
        Ok(Some(RdpRendererWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_rdp_renderer_writer_runtime_state(
        &self,
        epoch: &RdpRendererWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        pending_device_rsp_task: bool,
        pending_device_dpc_transaction: bool,
        pending_device_dp_completion: bool,
        pending_abi_renderer_work: bool,
    ) -> Result<
        Option<ValidatedRdpRendererWriterRuntimeStateReceiptV1>,
        RdpRendererWriterRuntimeStateErrorV1,
    > {
        if self.rdp_renderer_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.rdp_renderer_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let trace = RDP_RENDERER_WRITER_TRACE.with(|trace| {
            trace
                .borrow()
                .clone()
                .ok_or(RdpRendererWriterRuntimeStateErrorV1::TraceEpochNotArmed)
        })?;
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RdpRendererWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_rdp_renderer_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            epoch,
            storage,
            &state,
            &trace,
            pending_device_rsp_task,
            pending_device_dpc_transaction,
            pending_device_dp_completion,
            pending_abi_renderer_work,
        )?;
        RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
        self.rdp_renderer_writer_trace_epoch_id.set(None);
        self.rdp_renderer_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    pub(super) fn begin_pi_writer_runtime_trace_epoch(
        &self,
        pending_device_pi: bool,
        pending_abi_pi: bool,
        pending_pi_interrupt: bool,
    ) -> Result<Option<PiWriterRuntimeTraceEpochV1>, PiWriterRuntimeStateErrorV1> {
        if self.pi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(PiWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(PiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(PiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(PiWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(PiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_pi_writer_quiescence(&state)?;
        if pending_device_pi {
            return Err(PiWriterRuntimeStateErrorV1::PendingDevicePi);
        }
        if pending_abi_pi {
            return Err(PiWriterRuntimeStateErrorV1::PendingAbiPi);
        }
        if pending_pi_interrupt {
            return Err(PiWriterRuntimeStateErrorV1::PendingPiInterrupt);
        }
        drop(state);

        let epoch_id = next_pi_writer_trace_epoch_id();
        self.pi_writer_trace_epoch_id.set(Some(epoch_id));
        Ok(Some(PiWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_pi_writer_runtime_state(
        &self,
        epoch: &PiWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        trace: &[fn64_runtime::DeviceTraceEvent],
        pending_device_pi: bool,
        pending_abi_pi: bool,
    ) -> Result<Option<ValidatedPiWriterRuntimeStateReceiptV1>, PiWriterRuntimeStateErrorV1> {
        if self.pi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.pi_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(PiWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(PiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_pi_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            trace,
            pending_device_pi,
            pending_abi_pi,
        )?;
        self.pi_writer_trace_epoch_id.set(None);
        self.pi_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_si_writer_runtime_state(
        &self,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        trace: &[fn64_runtime::DeviceTraceEvent],
        pending_device_si: bool,
        pending_abi_si: bool,
    ) -> Result<Option<ValidatedSiWriterRuntimeStateReceiptV1>, SiWriterRuntimeStateErrorV1> {
        if self.si_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(SiWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(SiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_si_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            storage,
            &state,
            trace,
            pending_device_si,
            pending_abi_si,
        )?;
        self.si_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    pub(super) fn begin_sp_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<SpWriterRuntimeTraceEpochV1>, SpWriterRuntimeStateErrorV1> {
        if self.sp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(SpWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(SpWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(SpWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(SpWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(SpWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        if !state.sealed || state.expected_sha256.is_none() {
            return Err(SpWriterRuntimeStateErrorV1::Unsealed);
        }
        if state.poison.is_some() {
            return Err(SpWriterRuntimeStateErrorV1::Poisoned);
        }
        match pending_executable_write_violation() {
            Some(PendingWriteViolation::Physical) => return Err(SpWriterRuntimeStateErrorV1::PendingPhysicalWrites),
            Some(PendingWriteViolation::Attributed) => return Err(SpWriterRuntimeStateErrorV1::PendingAttributedWrites),
            None => {}
        }
        if !state.host_transactions.is_empty() {
            return Err(SpWriterRuntimeStateErrorV1::OpenHostTransactions);
        }
        if state.active_child_transaction.is_some() {
            return Err(SpWriterRuntimeStateErrorV1::ActiveChildTransaction);
        }
        drop(state);

        let epoch_id = next_sp_writer_trace_epoch_id();
        self.sp_writer_trace_epoch_id.set(Some(epoch_id));
        Ok(Some(SpWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_sp_writer_runtime_state(
        &self,
        epoch: &SpWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        trace: &[fn64_runtime::DeviceTraceEvent],
        pending_device_sp_dma: bool,
        pending_device_sp_task: bool,
        pending_abi_sp_work: bool,
    ) -> Result<Option<ValidatedSpWriterRuntimeStateReceiptV1>, SpWriterRuntimeStateErrorV1> {
        if self.sp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.sp_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(SpWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(SpWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_sp_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            trace,
            pending_device_sp_dma,
            pending_device_sp_task,
            pending_abi_sp_work,
        )?;
        self.sp_writer_trace_epoch_id.set(None);
        self.sp_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    pub(super) fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        if let Some(generations) = &self.generations {
            return self
                .install
                .resolve_entry_with_generations(target_pc, &generations.borrow());
        }
        self.install.resolve_entry(target_pc)
    }

    pub(super) fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if let Some(generations) = &self.generations {
            return self.install.resolve_transfer_with_generations(
                source_bank,
                target_pc,
                &generations.borrow(),
            );
        }
        self.install.resolve_transfer(source_bank, target_pc)
    }

    pub(super) fn resolve_call(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<CatalogCallResolutionV1, CpuFault> {
        if let Some(host) = self.install.resolve_host(target_pc.get()) {
            Ok(CatalogCallResolutionV1::Host(host))
        } else {
            self.resolve_transfer(source_bank, target_pc)
                .map(CatalogCallResolutionV1::Guest)
        }
    }

    pub(super) fn dispatch_exposing_exceptions_at_budget(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> Result<fn64_recomp_rs::DispatchRun, fn64_recomp_rs::DispatchError> {
        if let Some(generations) = &self.generations {
            return self
                .install
                .dispatch_exposing_exceptions_with_generations_at_budget(
                    entry,
                    &generations.borrow(),
                    budget,
                    ctx,
                    mem,
                );
        }
        self.install
            .dispatch_exposing_exceptions_at_budget(entry, budget, ctx, mem)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    pub(super) fn reserves_bank(&self, bank: BankId) -> bool {
        if let Some(generations) = &self.generations {
            return self
                .install
                .reserves_bank_with_generations(bank, &generations.borrow());
        }
        self.install.reserves_bank(bank)
    }

    pub(super) fn activate_for_fetch(
        &self,
        target_pc: GuestPc,
        mem: &Rdram<'_>,
    ) -> Result<ExecutionKey, GenerationLookupError> {
        self.generations
            .as_ref()
            .ok_or(GenerationLookupError::UnmappedPc { pc: target_pc })?
            .borrow_mut()
            .activate_for_fetch(target_pc, mem)
            .map(|resolution| resolution.entry)
    }

    /// Snapshot the watched region so a C shim's writes can be declared.
    ///
    /// Generated C shims receive a raw `rdram` pointer and write guest memory
    /// directly, below every attributed store path, so nothing declares for
    /// them. A raw-write watch caught exactly that: a single-byte store to
    /// `0x0009b0b3` from `call_c` with no covering declaration, which the next
    /// dispatch correctly reported as an unjournaled mutation.
    ///
    /// Returns `None` when there is no mutation state to declare against.
    pub(super) fn snapshot_for_host_shim(&self, mem: &Rdram<'_>) -> Option<Vec<Vec<u8>>> {
        let state = self.mutation_state.as_ref()?;
        if !state.borrow().sealed {
            return None;
        }
        let view = fn64_runtime::RdramView::from_storage(mem.as_slice());
        Some(state.borrow().read_snapshot_from_view(&view))
    }

    /// Declare every watched byte a C shim changed, as `HostAbi`.
    ///
    /// Pairs with [`Self::snapshot_for_host_shim`] taken before the call. The
    /// shim's own writes are invisible to attribution, so the diff across the
    /// call IS the declaration.
    pub(super) fn declare_host_shim_writes(&self, before: Option<Vec<Vec<u8>>>, mem: &Rdram<'_>) {
        let (Some(before), Some(state)) = (before, self.mutation_state.as_ref()) else {
            return;
        };
        let view = fn64_runtime::RdramView::from_storage(mem.as_slice());
        let after = state.borrow().read_snapshot_from_view(&view);
        // Compare against the pre-call snapshot rather than `expected`: only
        // what THIS shim changed belongs to it.
        let mut changed = Vec::new();
        for ((range, before_bytes), after_bytes) in
            state.borrow().watched.iter().zip(&before).zip(&after)
        {
            if before_bytes == after_bytes {
                continue;
            }
            let mut index = 0usize;
            while index < after_bytes.len() {
                if before_bytes[index] == after_bytes[index] {
                    index += 1;
                    continue;
                }
                let start = index;
                while index < after_bytes.len() && before_bytes[index] != after_bytes[index] {
                    index += 1;
                }
                changed.push((
                    range.physical_start + start as u32,
                    range.physical_start + index as u32,
                ));
            }
        }
        for (start, end) in changed {
            fn64_recomp_rs::notify_host_abi_write(start, end - start);
        }
    }

    pub(super) fn reconcile_before_dispatch(&self, mem: &Rdram<'_>) {
        let Some(state) = &self.mutation_state else {
            return;
        };
        // This runs at EVERY dispatch boundary over the 1 MiB boot bank, and
        // profiling put it at 1055 of 2627 samples once the hash moved to the
        // hardware backend. `reconcile_before_dispatch_with` reads through a
        // per-byte closure -- a bounds check and a lane XOR per byte -- which
        // `read_snapshot_from_view` already replaces with a word-wise copy.
        // Its own doc comment says the hot paths should use it; this is the
        // hot path.
        // Sealing must always happen: it establishes the baseline the journal
        // and the receipts are bound to, and it early-returns once sealed.
        state
            .borrow_mut()
            .seal_with(|physical| mem.load_bu(0xffff_ffff_8000_0000 | u64::from(physical)));
        // The COMPARISON is what costs. It only asserts that no undeclared
        // write occurred, which write attribution already guarantees, so it is
        // skippable in a lane meant to be played. See
        // `continuous_snapshot_enabled`.
        if !continuous_snapshot_enabled() {
            return;
        }
        let view = fn64_runtime::RdramView::from_storage(mem.as_slice());
        if state.borrow().reconcile_matched_before_dispatch(&view) {
            return;
        }
        let snapshot = state.borrow().read_snapshot_from_view(&view);
        state
            .borrow_mut()
            .reconcile_snapshot_before_dispatch(snapshot);
    }

    pub(super) fn reconcile_before_dispatch_with(&self, mut read_physical_byte: impl FnMut(u32) -> u8) {
        let Some(state) = &self.mutation_state else {
            return;
        };
        state.borrow_mut().seal_with(&mut read_physical_byte);
        let snapshot = state.borrow().read_snapshot(read_physical_byte);
        state
            .borrow_mut()
            .reconcile_snapshot_before_dispatch(snapshot);
    }

    /// [`Self::reconcile_before_dispatch_with`] with a word-wise snapshot.
    ///
    /// `seal_with` still needs a byte reader; only the per-dispatch 1 MiB
    /// snapshot moves to the view. For callers that hold the RDRAM allocation
    /// but not an `Rdram` -- the scheduler mirror in `execution.rs` is the hot
    /// one, running at every thread selection.
    pub(super) fn reconcile_before_dispatch_from_view(
        &self,
        view: &fn64_runtime::RdramView<'_>,
        mut read_physical_byte: impl FnMut(u32) -> u8,
    ) {
        let Some(state) = &self.mutation_state else {
            return;
        };
        state.borrow_mut().seal_with(&mut read_physical_byte);
        if state.borrow().reconcile_matched_before_dispatch(view) {
            return;
        }
        let snapshot = state.borrow().read_snapshot_from_view(view);
        state
            .borrow_mut()
            .reconcile_snapshot_before_dispatch(snapshot);
    }

    pub(super) fn begin_host_abi_transaction(
        &self,
        target: GuestPc,
        resume: ExecutionKey,
        mem: &Rdram<'_>,
    ) -> Option<HostMutationTransactionTokenV1> {
        let Some(state) = &self.mutation_state else {
            return None;
        };
        let thread = crate::current_thread_id("catalog host mutation transaction");
        if let Some(outer) = state.borrow().active_host_transaction(thread) {
            self.flush_host_abi_transaction(outer, mem);
        }
        self.reconcile_before_dispatch(mem);
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|events| events.borrow().len());
        assert_eq!(
            pending, 0,
            "catalog host transaction began with {pending} uncommitted child writer event(s)"
        );
        Some(
            state
                .borrow_mut()
                .begin_host_transaction(thread, target, resume),
        )
    }

    fn flush_host_abi_transaction_with(
        &self,
        token: HostMutationTransactionTokenV1,
        mut read_physical_byte: impl FnMut(u32) -> u8,
    ) {
        let state = self
            .mutation_state
            .as_ref()
            .expect("host transaction exists without canonical mutation state");
        state.borrow().assert_active_host_transaction(token);
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|events| events.borrow().len());
        assert_eq!(
            pending, 0,
            "catalog host transaction {} reached an ordering boundary with {pending} uncommitted child writer event(s)",
            token.transaction_id
        );
        let snapshot = state.borrow().read_snapshot(&mut read_physical_byte);
        let changed = state.borrow().current_changed_ranges(&snapshot);
        let first_new_entry = state.borrow().entries.len();
        for (physical_start, physical_end) in changed {
            fn64_recomp_rs::notify_host_abi_write(physical_start, physical_end - physical_start);
        }
        self.invalidate_pending_physical_writes_with(&mut read_physical_byte);
        state
            .borrow_mut()
            .record_host_abi_boundary(token, first_new_entry);
    }

    fn flush_host_abi_transaction(&self, token: HostMutationTransactionTokenV1, mem: &Rdram<'_>) {
        self.flush_host_abi_transaction_with(token, |physical| {
            mem.load_bu(0xffff_ffff_8000_0000 | u64::from(physical))
        });
    }

    pub(super) fn finish_host_abi_transaction(
        &self,
        token: Option<HostMutationTransactionTokenV1>,
        mem: &Rdram<'_>,
    ) {
        let Some(token) = token else {
            return;
        };
        self.flush_host_abi_transaction(token, mem);
        self.mutation_state
            .as_ref()
            .expect("host transaction lost canonical mutation state")
            .borrow_mut()
            .finish_host_transaction(token);
    }

    pub(super) fn flush_active_host_abi_transaction_with(
        &self,
        thread: ThreadId,
        read_physical_byte: impl FnMut(u32) -> u8,
    ) {
        let token = self
            .mutation_state
            .as_ref()
            .and_then(|state| state.borrow().active_host_transaction(thread));
        if let Some(token) = token {
            self.flush_host_abi_transaction_with(token, read_physical_byte);
        }
    }

    pub(super) fn invalidate_pending_physical_writes(&self, mem: &Rdram<'_>) -> Vec<GenerationId> {
        // The twin of `reconcile_before_dispatch`: it runs at the SAME dispatch
        // boundaries and used to take the same per-byte snapshot -- a bounds
        // check and a lane XOR for each of 1,048,576 bytes. Fixing only the
        // reconcile side left half the cost in place.
        //
        // `mem` is right here, so read the snapshot word-wise and hand the
        // slower closure path only the callers that genuinely have nothing but
        // a byte reader.
        let view = fn64_runtime::RdramView::from_storage(mem.as_slice());
        self.invalidate_pending_physical_writes_from_view(&view, |physical| {
            mem.load_bu(0xffff_ffff_8000_0000 | u64::from(physical))
        })
    }

    /// Same contract as [`Self::invalidate_pending_physical_writes_with`], but
    /// snapshots word-wise from an RDRAM view.
    ///
    /// `seal_with` still needs a byte reader, so that stays a closure; only the
    /// per-dispatch 1 MiB snapshot moves to the view.
    pub(super) fn invalidate_pending_physical_writes_from_view(
        &self,
        view: &fn64_runtime::RdramView<'_>,
        read_physical_byte: impl FnMut(u32) -> u8,
    ) -> Vec<GenerationId> {
        self.invalidate_pending_physical_writes_inner(read_physical_byte, Some(view))
    }

    pub(super) fn invalidate_pending_physical_writes_with(
        &self,
        read_physical_byte: impl FnMut(u32) -> u8,
    ) -> Vec<GenerationId> {
        self.invalidate_pending_physical_writes_inner(read_physical_byte, None)
    }

    fn invalidate_pending_physical_writes_inner(
        &self,
        mut read_physical_byte: impl FnMut(u32) -> u8,
        view: Option<&fn64_runtime::RdramView<'_>>,
    ) -> Vec<GenerationId> {
        let writes =
            PENDING_EXECUTABLE_WRITES.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        let events = PENDING_ATTRIBUTED_EXECUTABLE_WRITES
            .with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        let mut invalidated = Vec::new();
        if let Some(generations) = &self.generations {
            let mut generations = generations.borrow_mut();
            for &(physical_start, byte_len) in &writes {
                let physical_end = physical_start.checked_add(byte_len).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical generation write range overflows: {physical_start:#010x} + {byte_len:#x}"
                    ))
                });
                invalidated.extend(
                    generations
                        .invalidate_physical_write(physical_start, physical_end)
                        .unwrap_or_else(|error| {
                            recompiled_gap_panic(format!(
                                "canonical generation write range is invalid: {error}"
                            ))
                        }),
                );
            }
        } else if self.mutation_state.is_none() {
            assert!(
                writes.is_empty() && events.is_empty(),
                "catalog without executable backing retained attributed writes"
            );
            return Vec::new();
        }
        invalidated.sort_unstable();
        invalidated.dedup();
        if let Some(state) = &self.mutation_state {
            state.borrow_mut().seal_with(&mut read_physical_byte);
            // Reading the snapshot copies EVERY watched byte out of RDRAM, and
            // the commit below then SHA-256s that copy. Both callers run this on
            // every dispatch, so when neither has anything pending the pair was
            // ~42% of the shell's profile against 1 sample of actual guest
            // execution.
            //
            // NOTE: gating this read on "does some queued write intersect a
            // watched range" is WRONG and was reverted. It assumes `writes`
            // enumerates every mutation of watched memory, and it does not --
            // at least one path reaches RDRAM without passing through
            // `record_executable_and_renderer_write`. Skipping the read on that
            // assumption leaves `expected` stale, and the next dispatch fails
            // as "unjournaled executable mutation changed physical RDRAM
            // [0x0009b0b3, 0x0009b0b4) before canonical static dispatch" --
            // the same byte as the original Blocker A panic, in the gate lane
            // at 200k steps. The unconditional read is what makes the snapshot
            // the source of truth rather than the write queue.
            // With nothing declared, this read exists only to discover an
            // UNDECLARED change -- the same assertion `reconcile_before_dispatch`
            // makes, and the one write attribution already covers. Measured
            // over WM2000's full route, 505,140 journal entries contained zero
            // changed ranges without a covering declaration.
            //
            // When writes or events ARE pending the read is mandatory: it is
            // what advances the baseline and journals the entry the receipts
            // bind to, so it runs regardless of this flag.
            // NOTE: this read is NOT skippable when the journal is off.
            //
            // It looks like a pure "did anything change undeclared" check, but
            // it also ADVANCES the baseline: `adopt_snapshot` accepts the
            // current bytes as `expected`. Skipping it leaves the baseline
            // stale, and a later dispatch re-detects a change that was already
            // accepted -- the exact failure 6a2c330 diagnosed, and it
            // reproduced here as
            // "unjournaled executable mutation changed physical RDRAM
            //  [0x0009b0b3, 0x0009b0b4)" at 3M steps under
            // FN64_FAST_MUTATION_JOURNAL=1.
            //
            // The reconcile check in `reconcile_before_dispatch` IS skippable,
            // because it only compares and never advances anything. That one
            // stays gated; this one does not.
            // Nothing to attribute AND the live bytes still equal the
            // baseline: `adopt_snapshot` below is then provably a no-op, since
            // its first act is to return early when `expected` already equals
            // the snapshot. Skipping the read is skipping a copy whose only
            // consumer would discard it.
            //
            // This is NOT the reverted write-queue gate. That one asked "does
            // some queued write intersect a watched range" and skipped on the
            // false premise that the queue enumerates every mutation -- so an
            // undeclared write left `expected` stale and resurfaced later as
            // the 0x0009b0b3 panic. This asks RDRAM ITSELF whether anything
            // changed, which is the same source of truth the unconditional
            // read consults. If any byte differs -- declared or not -- the
            // match fails and the full read, commit and baseline advance run
            // exactly as before. The snapshot stays the source of truth; only
            // the copy it would have produced is elided.
            if writes.is_empty() && events.is_empty() {
                if let Some(view) = view {
                    if state.borrow().matches_view(view) {
                        return invalidated;
                    }
                }
            }
            let snapshot = match view {
                Some(view) => state.borrow().read_snapshot_from_view(view),
                None => state.borrow().read_snapshot(read_physical_byte),
            };
            // Skip a commit that has nothing to say. Both the guest-execution
            // path and the device-time advance
            // (`process_live_executable_writes_from_host`) call this same
            // method on the same canonical state, and each `std::mem::take`s
            // the shared PENDING_ATTRIBUTED_EXECUTABLE_WRITES queue.
            //
            // Whichever runs first consumes the declaration and refreshes
            // `watched[..].expected`. The second then arrives with an EMPTY
            // event list, and if it commits anyway it re-reads RDRAM, sees the
            // byte the first commit already accepted, and panics with
            // `events=0 declarations=0` -- which is exactly the WM2000 failure
            // at 0x8009b0b0.
            //
            // A commit with no writes and no events cannot establish anything,
            // so declining it is not a weakening: the first commit already
            // recorded the journal entry and advanced the baseline.
            if writes.is_empty() && events.is_empty() {
                // Nothing to attribute, but the baseline still has to advance.
                //
                // Both the guest-execution path and the device-time advance
                // (`process_live_executable_writes_from_host`) call this on the
                // SAME canonical state, and each `std::mem::take`s the shared
                // PENDING_ATTRIBUTED_EXECUTABLE_WRITES queue. The first
                // consumes the declaration and refreshes
                // `watched[..].expected`; the second arrives empty.
                //
                // Committing anyway made the second re-detect the byte the
                // first accepted (`events=0 declarations=0`). Skipping the
                // commit entirely just moved the same stale baseline to the
                // next `reconcile_snapshot_before_dispatch`, which reports it
                // as "before canonical static dispatch" -- measured, not
                // assumed. Adopting the snapshot without journalling an empty
                // batch is what actually keeps the two callers consistent.
                state.borrow_mut().adopt_snapshot(snapshot);
            } else {
                state
                    .borrow_mut()
                    .commit_snapshot(snapshot, events, invalidated.clone());
            }
        }
        invalidated
    }

    pub(super) fn mutation_evidence_snapshot(&self) -> Option<CanonicalExecutableMutationJournalEvidenceV1> {
        self.mutation_state
            .as_ref()
            .map(|state| state.borrow().evidence_snapshot())
    }

    pub(super) fn generation_evidence_snapshot(&self) -> Option<BackedGenerationCatalogEvidenceV1> {
        self.generations
            .as_ref()
            .map(|generations| generations.borrow().evidence_snapshot())
    }
}
