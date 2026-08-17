use fn64_render::{
    ir::{
        BackendCompletionAuthority, BackendEffectReport, RawCommandStream, ResourceRegion,
        SubmittedTicket, WorkloadAdmission,
    },
    DpFullSyncStatus, IrGuestMemorySnapshot, IrRawDpcBackendCompletion, RenderBackend, RenderError,
    StagedIrRdramWrite,
};

use super::ReferenceBackend;

impl ReferenceBackend {
    fn begin_ir_rdram_write_trace(&mut self) {
        assert!(
            self.ir_rdram_write_trace.replace(Vec::new()).is_none(),
            "reference IR RDRAM write trace was already active"
        );
    }

    fn take_ir_rdram_write_trace(&mut self) -> Vec<(usize, usize)> {
        self.ir_rdram_write_trace
            .take()
            .expect("reference IR RDRAM write trace was not active")
    }
}

/// Renderer-owned IR completion role for the first raw-DPC integration slice.
///
/// Execution is deliberately narrow and stateless across packets: one or more
/// owned DRAM streams run through a clone of one template backend against a
/// complete shadow image, then that clone is discarded even on success. This
/// slice therefore cannot publish unreceipted persistent RDP/TMEM/hidden-bit
/// state. XBUS staging, persistent backend transactions, and production
/// device scheduling remain on the existing ABI path.
pub struct ReferenceIrRawDpcAdapter {
    backend_template: ReferenceBackend,
    completion_authority: BackendCompletionAuthority,
}

impl ReferenceIrRawDpcAdapter {
    pub const fn new(
        backend: ReferenceBackend,
        completion_authority: BackendCompletionAuthority,
    ) -> Self {
        Self {
            backend_template: backend,
            completion_authority,
        }
    }

    pub const fn backend(&self) -> &ReferenceBackend {
        &self.backend_template
    }

    /// Execute one submitted raw-DPC packet transactionally and issue the
    /// matching backend-effect receipt.
    ///
    /// A rejection drops the ephemeral submitted ticket, speculative backend,
    /// and staged bytes together. No guest byte or persistent reference state
    /// changes before the returned completion reaches a guest-memory owner.
    pub fn execute(
        &mut self,
        submitted: SubmittedTicket,
        guest_snapshot: IrGuestMemorySnapshot,
        output_addr: u32,
    ) -> Result<IrRawDpcBackendCompletion, RenderError> {
        let packet = submitted.packet();
        if !matches!(packet.admission(), WorkloadAdmission::RawDpc { .. }) {
            return Err(adapter_error("packet admission is not raw DPC"));
        }
        if guest_snapshot.preimage().queue() != submitted.queue() {
            return Err(adapter_error(
                "guest snapshot belongs to a different lifecycle queue",
            ));
        }
        if guest_snapshot.preimage().submission() != submitted.identity()
            || guest_snapshot.preimage().submission_ordinal() != submitted.ordinal()
        {
            return Err(adapter_error(
                "guest snapshot belongs to a different submitted workload",
            ));
        }
        if packet.memory_layout().bytes() != guest_snapshot.preimage().byte_len() {
            return Err(adapter_error(format!(
                "packet binds {:#x} bytes but the guest snapshot has {:#x}",
                packet.memory_layout().bytes(),
                guest_snapshot.preimage().byte_len()
            )));
        }

        let guest_preimage = guest_snapshot.preimage();
        let mut shadow = guest_snapshot.bytes().to_vec();
        let mut speculative = self.backend_template.clone();
        // File-backed diagnostics are renderer state rather than globals.
        // Remove the sink before entering speculative execution; a file
        // cannot be made transactional by deleting it after rejection.
        speculative.auto_dump.take();
        #[cfg(not(test))]
        {
            speculative.suppress_task_diagnostics = true;
        }
        let write_accesses = packet
            .journal()
            .accesses()
            .iter()
            .copied()
            .filter(|access| access.mode().writes())
            .collect::<Vec<_>>();
        let mut declared_ranges = Vec::with_capacity(write_accesses.len());
        for access in &write_accesses {
            let ResourceRegion::Rdram { range, .. } = access.region() else {
                return Err(adapter_error(
                    "the first reference IR adapter receipts only RDRAM writes",
                ));
            };
            declared_ranges.push((range.start().get() as usize, range.end() as usize));
        }
        for stream in packet.streams() {
            let RawCommandStream::Dram(stream) = stream else {
                return Err(adapter_error(
                    "the first reference IR adapter does not stage XBUS streams",
                ));
            };
            // `DramCommandStream::try_new` rejects
            // `DiscontiguousCommandChunks`, so this combined range contains
            // only owned chunk bytes and cannot execute a guest-memory gap.
            debug_assert!(stream
                .chunks()
                .windows(2)
                .all(|pair| pair[0].range().end() == pair[1].range().start().get()));
            let end = stream
                .chunks()
                .last()
                .expect("IR command stream is nonempty")
                .range()
                .end();
            let terminator_start = end as usize;
            let terminator_end = terminator_start
                .checked_add(8)
                .filter(|&end| end <= shadow.len())
                .ok_or_else(|| {
                    adapter_error("reference raw-DPC terminator scratch exceeds installed RDRAM")
                })?;
            if declared_ranges
                .iter()
                .any(|&(start, end)| terminator_start < end && start < terminator_end)
            {
                return Err(adapter_error(
                    "the first reference IR adapter requires terminator scratch disjoint from declared writes",
                ));
            }
        }

        let (execution, unsupported_attempts) = crate::without_speculative_observations(|| {
            for stream in packet.streams() {
                let RawCommandStream::Dram(stream) = stream else {
                    unreachable!("all streams were preflighted as DRAM")
                };
                let start = stream.chunks()[0].range().start().get();
                let end = stream
                    .chunks()
                    .last()
                    .expect("IR command stream is nonempty")
                    .range()
                    .end();

                // Each capture is installed into a fresh per-call image only
                // immediately before that call. Comparing against the same
                // installed baseline separates renderer effects from command
                // staging. This preserves an earlier write even when a later
                // immutable stream occupies overlapping guest addresses.
                let mut stream_image = shadow.clone();
                install_owned_dram_command_stream(stream, &mut stream_image);
                let installed = stream_image.clone();
                let terminator_start = end as usize;
                let terminator_end = terminator_start + 8;
                let retained_terminator_bytes =
                    stream_image[terminator_start..terminator_end].to_vec();
                speculative.begin_ir_rdram_write_trace();
                let status = speculative.process_rdp_commands(
                    &mut stream_image,
                    start,
                    end,
                    output_addr,
                    true,
                )?;
                // `process_rdp_commands` copies its synthetic G_ENDDL parser
                // marker back with the complete image. That marker is not an
                // RDP effect; scratch/write aliasing was rejected in preflight.
                stream_image[terminator_start..terminator_end]
                    .copy_from_slice(&retained_terminator_bytes);
                let written_ranges = speculative.take_ir_rdram_write_trace();
                if status != fn64_render::FrameStatus::Complete {
                    return Err(adapter_error(format!(
                        "reference raw-DPC execution returned nonterminal status {status:?}"
                    )));
                }
                let expected_full_sync = if stream.full_sync_occurrences().is_empty() {
                    DpFullSyncStatus::NotReached
                } else {
                    DpFullSyncStatus::Reached
                };
                if speculative.last_dp_full_sync() != expected_full_sync {
                    return Err(adapter_error(format!(
                        "reference FullSync result {:?} disagrees with packet {:?}",
                        speculative.last_dp_full_sync(),
                        expected_full_sync
                    )));
                }

                for &(start, end) in &written_ranges {
                    if end > shadow.len() {
                        return Err(adapter_error(format!(
                            "reference backend traced out-of-bounds RDRAM write [{start:#010x}, {end:#010x})"
                        )));
                    }
                    for offset in start..end {
                        if !declared_ranges
                            .iter()
                            .any(|&(start, end)| offset >= start && offset < end)
                        {
                            return Err(adapter_error(format!(
                                "reference backend wrote undeclared RDRAM byte {offset:#010x}"
                            )));
                        }
                        shadow[offset] = stream_image[offset];
                    }
                }
                if let Some(offset) = installed.iter().zip(&stream_image).enumerate().find_map(
                    |(offset, (before, after))| {
                        (before != after
                            && !written_ranges
                                .iter()
                                .any(|&(start, end)| offset >= start && offset < end))
                        .then_some(offset)
                    },
                ) {
                    return Err(adapter_error(format!(
                        "reference backend changed untraced RDRAM byte {offset:#010x}"
                    )));
                }
            }
            Ok(())
        });
        if let Some(attempt) = unsupported_attempts.first() {
            return Err(adapter_error(attempt.rejection_reason()));
        }
        execution?;

        let mut staged = Vec::with_capacity(write_accesses.len());
        for access in write_accesses {
            let ResourceRegion::Rdram { range, .. } = access.region() else {
                return Err(adapter_error(
                    "the first reference IR adapter receipts only RDRAM writes",
                ));
            };
            let start = range.start().get() as usize;
            let end = range.end() as usize;
            staged.push(
                StagedIrRdramWrite::try_new(access, shadow[start..end].to_vec())
                    .map_err(|error| adapter_error(error.to_string()))?,
            );
        }
        let completed_writes = staged
            .iter()
            .map(StagedIrRdramWrite::completed_write)
            .collect();
        let report = BackendEffectReport::try_new(packet, completed_writes)
            .map_err(|error| adapter_error(error.to_string()))?;
        let receipt = self
            .completion_authority
            .issue(&submitted, report)
            .map_err(|error| adapter_error(error.to_string()))?;
        let complete = submitted
            .gpu_complete(receipt)
            .map_err(|error| adapter_error(error.to_string()))?;
        let completion = IrRawDpcBackendCompletion::try_new(complete, guest_preimage, staged)
            .map_err(|error| adapter_error(error.to_string()))?;
        // Persistent renderer state is deliberately not part of this first
        // packet receipt, so the speculative backend is discarded on success
        // exactly as it is on rejection.
        Ok(completion)
    }
}

fn install_owned_dram_command_stream(
    stream: &fn64_render::ir::DramCommandStream,
    shadow: &mut [u8],
) {
    for chunk in stream.chunks() {
        let start = chunk.range().start().get() as usize;
        for (index, word) in chunk.words().iter().copied().enumerate() {
            let offset = start + index * size_of::<u32>();
            shadow[offset..offset + size_of::<u32>()].copy_from_slice(&word.to_ne_bytes());
        }
    }
}

fn adapter_error(reason: impl Into<String>) -> RenderError {
    RenderError::Backend {
        backend: "reference-render-ir",
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn speculative_guard_journals_unsupported_and_blocks_zstat_publication() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        crate::raster::zstat::test_enable_and_reset();

        let ((), attempts) = crate::without_speculative_observations(|| {
            crate::record_render_unsupported(
                "render.test.speculative",
                "transaction-local rejection probe",
                fn64_runtime::UnsupportedDisposition::ReturnedError,
            );
            crate::raster::zstat::note_pass();
            crate::raster::zstat::note_reject();
        });

        assert_eq!(attempts.len(), 1);
        assert!(attempts[0]
            .rejection_reason()
            .contains("render.test.speculative"));
        assert!(fn64_runtime::copy_unsupported_events().is_empty());
        assert_eq!(crate::raster::zstat::test_counts(), (0, 0));
    }
}
