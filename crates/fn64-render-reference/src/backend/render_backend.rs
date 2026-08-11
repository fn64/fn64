// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::raster::Framebuffer;
use crate::{
    gbi, vi,
};
use fn64_render::{
    FrameStatus, MicrocodeDataImageIdentity,
    NonRdpWrite16, NonRdpWrite16Disposition, OsTask, PresentMemory, PresentRequest, RenderBackend,
    RenderConfig, RenderError, UcodeId, ViPresentation,
};

use super::*;
use super::hidden_bits::*;
use super::vi_source::*;
use sha2::Digest;

impl RenderBackend for ReferenceBackend {
    fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
        self.active_tv_type.map_or(
            fn64_render::RenderBackendEvidence::Unidentified,
            |tv_type| fn64_render::RenderBackendEvidence::Reference { tv_type },
        )
    }

    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        self.active_tv_type = None;
        let mut fb = Framebuffer::new(cfg.width, cfg.height);
        fb.set_noise_seed(self.noise_seed);
        let [r, g, b, a] = self.clear_color;
        fb.clear(r, g, b, a);
        self.presented_fb = Some(fb.clone());
        self.presentation = ViPresentation::default();
        self.fb = Some(fb);
        self.color_image = None;
        self.depth_image = None;
        self.primitive_depth = None;
        self.rdp_decode_state = gbi::RdpDecodeState::default();
        self.rdram_hidden_bits.clear();
        self.continuation = None;
        self.next_continuation_token = 1;
        self.active_tv_type = Some(cfg.tv_type);
        Ok(())
    }

    fn observe_non_rdp_write16(&mut self, write: NonRdpWrite16) -> NonRdpWrite16Disposition {
        let address = write.logical_offset().offset();
        if self.rdram_hidden_bits.contains_key(&address) {
            record_non_rdp_16bit_write(&mut self.rdram_hidden_bits, address, write.value());
            NonRdpWrite16Disposition::AppliedHiddenSidecar
        } else {
            NonRdpWrite16Disposition::NoRustHiddenSidecar
        }
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        let mut state = match self.prepare_reference_task(rdram, rsp_memory, task, output_addr)? {
            PreparedReferenceTask::Ready(state) => state,
            PreparedReferenceTask::NeedsLle(ucode_sha256) => {
                return Ok(FrameStatus::NeedsLle { ucode_sha256 });
            }
        };
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        while state.next_operation < state.operations.len() {
            let operation = state.operations[state.next_operation].clone();
            state.next_operation += 1;
            self.execute_reference_operation(rdram, &mut state, &operation)?;
            state.reached_dp_full_sync |= matches!(operation, gbi::RenderOp::FullSync);
        }
        let dp_full_sync = if state.reached_dp_full_sync {
            fn64_render::DpFullSyncStatus::Reached
        } else {
            fn64_render::DpFullSyncStatus::NotReached
        };
        // This trait call is atomic with respect to guest execution: unlike
        // `process_task_chunk`, it publishes no continuation at which SIG0 or
        // another guest thread can observe RDRAM. Target changes and FullSync
        // still commit inside `execute_reference_operation`; the remaining
        // dirty image needs one commit at the task boundary.
        self.finish_reference_task(rdram, state)?;
        self.last_dp_full_sync = dp_full_sync;
        Ok(FrameStatus::Complete)
    }

    fn process_task_chunk(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
        step: fn64_render::RenderTaskStep,
    ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
        self.process_reference_task_chunk(rdram, rsp_memory, task, output_addr, step)
    }

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        _output_addr: u32,
        _wait_for_completion: bool,
    ) -> Result<FrameStatus, RenderError> {
        // The software rasterizer has no async completion to defer -- every
        // call is synchronous CPU work, so the flag is accepted (never
        // ignored silently -- it is a named parameter) and always honored
        // as `true` per the trait's documented fallback.
        gbi::validate_raw_rdp_command_range(rdram, start, end)?;
        let terminated_len = (end as usize)
            .checked_add(8)
            .ok_or_else(|| RenderError::Backend {
                backend: "reference",
                reason: "raw RDP terminator address overflow".to_string(),
            })?;
        let mut image = rdram.to_vec();
        image.resize(terminated_len.max(image.len()), 0);
        image[end as usize..end as usize + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
        image[end as usize + 4..end as usize + 8].copy_from_slice(&0u32.to_ne_bytes());

        let previous_mode = self.decode_mode;
        self.decode_mode = DecodeMode::RawRdp;
        let result = self.process_task(
            &mut image,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: start,
                ..OsTask::default()
            },
            0,
        );
        self.decode_mode = previous_mode;
        if result.is_ok() {
            rdram.copy_from_slice(&image[..rdram.len()]);
        }
        result
    }

    fn raw_dpc_batch_capability(&self) -> fn64_render::RawDpcBatchCapability {
        fn64_render::RawDpcBatchCapability::DiagnosticOnly
    }

    fn process_raw_dpc_batch(
        &mut self,
        rdram: &mut [u8],
        batch: fn64_render::PreflightedRawDpcBatch,
        output_addr: u32,
    ) -> Result<fn64_render::RawDpcBatchOutcome, RenderError> {
        let expected_full_sync = batch.aggregate_full_sync();
        let outcome = batch.outcome();
        let groups = batch.stream_groups().to_vec();
        let mut image = batch.staged_image(rdram)?;
        let mut speculative = self.clone();
        // A diagnostic file cannot be rolled back if a later stream group
        // rejects. Retain the configured sink and its counters outside the
        // speculative backend, then restore them only at the batch commit.
        let retained_auto_dump = speculative.auto_dump.take();
        #[cfg(not(test))]
        {
            speculative.suppress_task_diagnostics = true;
        }
        for group in groups {
            let mut group_image = image.clone();
            let status = speculative.process_rdp_commands(
                &mut group_image,
                group.staging_start(),
                group.staging_end(),
                output_addr,
                true,
            )?;
            if status != FrameStatus::Complete {
                return Err(RenderError::Backend {
                    backend: "reference-raw-dpc-batch",
                    reason: format!("raw-DPC stream group returned nonterminal status {status:?}"),
                });
            }
            if speculative.last_dp_full_sync() != group.full_sync() {
                return Err(RenderError::Backend {
                    backend: "reference-raw-dpc-batch",
                    reason: format!(
                        "renderer reported {:?} after group preflight proved {:?}",
                        speculative.last_dp_full_sync(),
                        group.full_sync()
                    ),
                });
            }
            image[..rdram.len()].copy_from_slice(&group_image[..rdram.len()]);
        }
        speculative.last_dp_full_sync = expected_full_sync;
        speculative.auto_dump = retained_auto_dump;
        #[cfg(not(test))]
        {
            speculative.suppress_task_diagnostics = false;
        }
        rdram.copy_from_slice(&image[..rdram.len()]);
        *self = speculative;
        Ok(outcome)
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.last_dp_full_sync
    }

    fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
        fn64_render::RenderTaskChunking::Resumable
    }

    fn present(&mut self, request: PresentRequest<'_>) -> Result<(), RenderError> {
        let (vi, memory) = request.into_parts();
        let resident = self
            .fb
            .as_ref()
            .ok_or(RenderError::NotReady("create() not called"))?;
        let (presented, hidden_updates) = match memory {
            PresentMemory::BackendResidentCompatibility => (vi::scanout(resident, vi)?, Vec::new()),
            PresentMemory::Physical(memory) => {
                if vi.scanout.registers().is_none() {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "physical VI presentation requires a live register image"
                            .to_string(),
                    });
                }
                match reference_vi_source_geometry(vi)? {
                    Some(geometry) => {
                        let (source, hidden_updates) =
                            load_vi_source(&memory, geometry, &self.rdram_hidden_bits)?;
                        (vi::scanout(&source, vi)?, hidden_updates)
                    }
                    None => (vi::scanout(resident, vi)?, Vec::new()),
                }
            }
        };
        self.presented_fb = Some(presented);
        self.presentation = vi;
        self.rdram_hidden_bits.extend(hidden_updates);
        Ok(())
    }

    fn resize(&mut self, w: u32, h: u32) {
        assert!(
            self.continuation.is_none(),
            "ReferenceBackend::resize cannot replace framebuffer storage while a render continuation is retained"
        );
        let clear_color = self.clear_color;
        if let Some(fb) = &mut self.fb {
            let mut new_fb = fb.resized(w, h);
            new_fb.clear(
                clear_color[0],
                clear_color[1],
                clear_color[2],
                clear_color[3],
            );
            *fb = new_fb;
        }
        if self.presentation.scanout.registers().is_some() {
            // A resize has no retrace-scoped RDRAM authority. Never rebuild a
            // live register image from the unrelated resident RDP surface;
            // the next field reconstructs it from current physical bytes.
            self.presented_fb = None;
        } else if let Some(fb) = &self.fb {
            // `resize` is infallible by trait contract. If the new dimensions
            // cannot support the retained VI effect, leave no fabricated
            // scanout; the next `present` reports the named error.
            self.presented_fb = vi::scanout(fb, self.presentation).ok();
        }
    }

    fn identify_microcode(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        let geometry = self.f3dex2_ucodes.identify_text(imem);
        let sprite = self.s2dex_ucodes.identify_text(imem);
        match (geometry, sprite) {
            (Some(geometry), Some(sprite)) => {
                panic!("one microcode digest cannot identify both {geometry:?} and {sprite:?}")
            }
            (Some(ucode), None) | (None, Some(ucode)) => Some(ucode),
            (None, None) => None,
        }
    }

    fn identify_microcode_pair(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        data: MicrocodeDataImageIdentity,
    ) -> Option<UcodeId> {
        self.microcode_pairs.identify(imem, data)
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        match self.decode_mode {
            DecodeMode::S2dex => self.s2dex_ucodes.supported_ucodes(),
            DecodeMode::F3dex2 => self.f3dex2_ucodes.supported_ucodes(),
            DecodeMode::Simple | DecodeMode::RawRdp => gbi::SUPPORTED,
        }
    }
}
