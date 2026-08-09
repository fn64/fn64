// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::raster::Framebuffer;
use crate::{
    gbi, png_dump, raster, s2dex, GeometryWireFamily,
    S2dexWireFamily,
};
use fn64_render::{
    F3dex2UcodeCatalog, MicrocodeDataImageIdentity, MicrocodePairCatalog, OsTask, RenderBackend, RenderError, S2dexUcodeCatalog, UcodeId, ViPresentation,
};

use super::*;
use super::hidden_bits::*;
use super::validate::*;
use super::framebuffer_io::*;
use sha2::Digest;

impl ReferenceBackend {
    pub fn new() -> Self {
        ReferenceBackend {
            active_tv_type: None,
            fb: None,
            presented_fb: None,
            presentation: ViPresentation::default(),
            color_image: None,
            depth_image: None,
            primitive_depth: None,
            rdp_decode_state: gbi::RdpDecodeState::default(),
            rdram_hidden_bits: RdramHiddenBits::new(),
            clear_color: [0, 0, 0, 255],
            noise_seed: Framebuffer::DEFAULT_NOISE_SEED,
            decode_mode: DecodeMode::Simple,
            f3dex2_ucodes: F3dex2UcodeCatalog::default(),
            s2dex_ucodes: S2dexUcodeCatalog::default(),
            microcode_pairs: MicrocodePairCatalog::default(),
            last_dp_full_sync: fn64_render::DpFullSyncStatus::Unidentified,
            auto_dump: None,
            #[cfg(not(test))]
            diag_task_index: 0,
            #[cfg(not(test))]
            suppress_task_diagnostics: false,
            continuation: None,
            next_continuation_token: 1,
        }
    }

    /// Select real F3DEX2 command decoding (matrix stack, segment table,
    /// viewport) instead of the simple reference-fixture encoding. This does
    /// not admit any microcode image: callers must also register every exact
    /// compatible text digest, or the task is replayed through LLE.
    pub fn with_f3dex2(mut self) -> Self {
        self.decode_mode = DecodeMode::F3dex2;
        self
    }

    /// Admit one exact task-entry or self-load target as F3DEX2-compatible.
    /// The digest is SHA-256 over the complete logical 4 KiB text image. This
    /// API carries identity rather than game bytes, so a host can configure
    /// known public variants without placing ROM or ucode content in fn64.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB F3DEX2 text image, retaining only its
    /// SHA-256 identity. Primarily useful to deterministic fixtures that
    /// construct synthetic IMEM rather than carrying a precomputed digest.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
        self
    }

    /// Admit one exact geometry-microcode digest with an explicit public wire
    /// family. Digest identity, never a colliding opcode, selects the decoder.
    pub fn with_geometry_ucode_sha256(
        mut self,
        family: GeometryWireFamily,
        digest: [u8; 32],
    ) -> Self {
        self.decode_mode = DecodeMode::F3dex2;
        self.f3dex2_ucodes.admit_sha256_for(family, digest);
        self
    }

    /// Admit one exact logical 4 KiB geometry-microcode text image with an
    /// explicit public wire family, retaining only its SHA-256 identity.
    pub fn with_geometry_ucode_text(mut self, family: GeometryWireFamily, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "geometry microcode text admission requires one complete 4 KiB IMEM image"
        );
        self.decode_mode = DecodeMode::F3dex2;
        self.f3dex2_ucodes.admit_text_for(family, text);
        self
    }

    /// Select the content-admitted S2DEX/S2DEX2 object decoder. This does not
    /// admit a text image or guess its wire family.
    pub fn with_s2dex(mut self) -> Self {
        self.decode_mode = DecodeMode::S2dex;
        self
    }

    /// Admit one exact 4 KiB S2DEX2 task-entry text identity.
    ///
    /// This source-compatible method predates the legacy S2DEX decoder and is
    /// deliberately defined as [`S2dexWireFamily::S2dex2`].
    pub fn with_s2dex_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.s2dex_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact task-entry identity with an explicit S2DEX wire family.
    pub fn with_s2dex_ucode_sha256_for(
        mut self,
        family: S2dexWireFamily,
        digest: [u8; 32],
    ) -> Self {
        self.s2dex_ucodes.admit_sha256_for(family, digest);
        self
    }

    /// Admit one exact logical 4 KiB S2DEX2 task-entry image, retaining only
    /// its SHA-256 identity. Intended for synthetic fixtures. Use
    /// [`Self::with_s2dex_ucode_text_for`] for legacy S2DEX.
    pub fn with_s2dex_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "S2DEX text admission requires one complete 4 KiB IMEM image"
        );
        self.s2dex_ucodes.admit_text(text);
        self
    }

    /// Admit one exact logical 4 KiB image with an explicit S2DEX wire family.
    pub fn with_s2dex_ucode_text_for(mut self, family: S2dexWireFamily, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "S2DEX text admission requires one complete 4 KiB IMEM image"
        );
        self.s2dex_ucodes.admit_text_for(family, text);
        self
    }

    /// Admit one exact complete microcode text/data identity for runtime
    /// recognition evidence. This is separate from HLE text admission.
    pub fn with_microcode_pair_sha256(
        mut self,
        family: UcodeId,
        text_sha256: [u8; 32],
        data_bytes: u32,
        data_sha256: [u8; 32],
    ) -> Self {
        self.microcode_pairs.admit(
            family,
            text_sha256,
            MicrocodeDataImageIdentity {
                bytes: data_bytes,
                sha256: data_sha256,
            },
        );
        self
    }

    /// Byte-backed fixture convenience for [`Self::with_microcode_pair_sha256`].
    pub fn with_microcode_pair(mut self, family: UcodeId, text: &[u8], data: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "microcode pair admission requires one complete 4 KiB IMEM image"
        );
        let data_bytes = u32::try_from(data.len())
            .expect("microcode pair data length exceeds the OSTask u32 size field");
        self.microcode_pairs.admit(
            family,
            sha2::Sha256::digest(text).into(),
            MicrocodeDataImageIdentity {
                bytes: data_bytes,
                sha256: sha2::Sha256::digest(data).into(),
            },
        );
        self
    }

    /// After each `process_task`, write the rasterized framebuffer to
    /// `<dir>/<prefix>-NNNN.png` (NNNN = the non-clear-frame counter),
    /// stopping after `limit` non-clear frames. This lets a harness recover
    /// the backend's output even after `set_render_backend` has taken
    /// ownership of it. Every dump (and every all-clear skip) is logged so a
    /// blank boot is reported honestly, never faked.
    pub fn with_auto_dump(
        mut self,
        dir: impl Into<std::path::PathBuf>,
        prefix: impl Into<String>,
        limit: u64,
    ) -> Self {
        self.auto_dump = Some(AutoDump {
            dir: dir.into(),
            prefix: prefix.into(),
            task_index: 0,
            skip_before_task: 0,
            written: 0,
            limit,
            limit_reported: false,
        });
        self
    }

    /// Start auto-dumping at gfx task index `first_task`.
    ///
    /// Call this after [`Self::with_auto_dump`]. Tasks before the requested
    /// index are still rendered and written back to guest RDRAM; only their
    /// diagnostic PNG output is suppressed.
    pub fn with_auto_dump_skip(mut self, first_task: u64) -> Self {
        self.auto_dump
            .as_mut()
            .expect("with_auto_dump_skip requires with_auto_dump first")
            .skip_before_task = first_task;
        self
    }

    /// Override the clear color a fresh/resized framebuffer starts from.
    /// Exposed mainly so tests can pick a clear color that's unambiguously
    /// distinct from any triangle color in a fixture, making "did geometry
    /// actually render" trivial to assert.
    pub fn with_clear_color(mut self, rgba: [u8; 4]) -> Self {
        self.clear_color = rgba;
        self
    }

    /// Select the reproducible reference stream used for combiner, RGB,
    /// alpha, and alpha-compare noise. This seed controls a host emulation
    /// policy; it is not the RDP's unpublished hardware seed.
    pub fn with_noise_seed(mut self, seed: u64) -> Self {
        self.noise_seed = seed;
        self
    }

    /// The current framebuffer's raw RGBA8888 pixels, for a test/harness to
    /// inspect or dump (`png_dump::write_png`). `None` before `create`.
    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        self.fb.as_ref()
    }

    /// The image produced by the most recent VI presentation boundary.
    /// Unlike [`Self::framebuffer`], this includes VI-level blanking.
    pub fn presented_framebuffer(&self) -> Option<&Framebuffer> {
        self.presented_fb.as_ref()
    }

    pub(super) fn allocate_continuation_token(&mut self) -> fn64_render::RenderTaskContinuation {
        let value = self.next_continuation_token;
        self.next_continuation_token = self
            .next_continuation_token
            .checked_add(1)
            .expect("reference render continuation token space exhausted");
        fn64_render::RenderTaskContinuation::new(value)
    }

    pub(super) fn prepare_reference_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<PreparedReferenceTask, RenderError> {
        if let Some(pending) = &self.continuation {
            return Err(RenderError::Backend {
                backend: "reference-task-continuation",
                reason: format!(
                    "cannot start a new task while continuation token {} is retained",
                    pending.token.get()
                ),
            });
        }
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        let (fb_width, fb_height) = self
            .fb
            .as_ref()
            .map(|fb| (fb.width, fb.height))
            .ok_or(RenderError::NotReady("create() not called"))?;

        // The public OSTask field is an end pointer, not a byte count.
        let out_start = (task.output_buff & 0x00FF_FFFF) as usize;
        let out_end = (task.output_buff_size & 0x00FF_FFFF) as usize;
        if task.output_buff_size != 0 && out_end > rdram.len() {
            return Err(RenderError::InvalidTaskBounds {
                offset: task.output_buff,
                len: out_end.saturating_sub(out_start) as u32,
                rdram_len: rdram.len(),
            });
        }

        let persistent_target = self.color_image;
        let persistent_depth_image = self.depth_image;
        let operations = match self.decode_mode {
            DecodeMode::Simple => gbi::decode_display_list(&*rdram, task.data_ptr)?
                .into_iter()
                .map(gbi::RenderOp::Triangle)
                .collect::<Vec<_>>(),
            DecodeMode::F3dex2 => {
                let family = match self
                    .f3dex2_ucodes
                    .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                {
                    Ok(family) => family,
                    Err(RenderError::RequiresLle { ucode_sha256 }) => {
                        return Ok(PreparedReferenceTask::NeedsLle(ucode_sha256));
                    }
                    Err(error) => return Err(error),
                };
                // HLE decode remains transactional: an unadmitted self-load
                // cannot leave partial RSP, RDRAM, or RDP-decode mutations.
                let mut speculative_rdram = rdram.to_vec();
                let mut speculative_rsp = rsp_memory.clone();
                let mut speculative_rdp = self.rdp_decode_state.clone();
                let operations =
                    match gbi::execute_display_list_geometry_ops_admitted_with_rdp_state(
                        &mut speculative_rdram,
                        &mut speculative_rsp,
                        task.data_ptr,
                        &self.f3dex2_ucodes,
                        &mut speculative_rdp,
                        family,
                    ) {
                        Ok(operations) => operations,
                        Err(RenderError::RequiresLle { ucode_sha256 }) => {
                            return Ok(PreparedReferenceTask::NeedsLle(ucode_sha256));
                        }
                        Err(error) => return Err(error),
                    };
                rdram.copy_from_slice(&speculative_rdram);
                *rsp_memory = speculative_rsp;
                self.rdp_decode_state = speculative_rdp;
                operations
            }
            DecodeMode::S2dex => {
                let family = match self
                    .s2dex_ucodes
                    .require_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                {
                    Ok(family) => family,
                    Err(RenderError::RequiresLle { ucode_sha256 }) => {
                        return Ok(PreparedReferenceTask::NeedsLle(ucode_sha256));
                    }
                    Err(error) => return Err(error),
                };
                let mut speculative_rdp = self.rdp_decode_state.clone();
                let operations = s2dex::decode_ops_for_family(
                    &*rdram,
                    task.data_ptr,
                    &mut speculative_rdp,
                    family,
                )?;
                self.rdp_decode_state = speculative_rdp;
                operations
            }
            DecodeMode::RawRdp => gbi::decode_raw_rdp_ops_with_state(
                &*rdram,
                task.data_ptr,
                &mut self.rdp_decode_state,
            )?,
        };
        let tri_count = operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation,
                    gbi::RenderOp::Triangle(_)
                        | gbi::RenderOp::Line(_)
                        | gbi::RenderOp::RawTriangle(_)
                )
            })
            .count();

        #[cfg(not(test))]
        {
            if !self.suppress_task_diagnostics {
                let dump_index = self.diag_task_index;
                self.diag_task_index += 1;
                if let Some(spec) = std::env::var_os("FN64_GFX_TASK_DUMP") {
                    let selected = spec.to_string_lossy().split(',').any(|entry| {
                        entry.trim().parse::<u64>().unwrap_or_else(|error| {
                            panic!(
                                "FN64_GFX_TASK_DUMP entry {entry:?} is not a u64 task index: {error}"
                            )
                        }) == dump_index
                    });
                    if selected {
                        let directory = std::env::var_os("FN64_GFX_TASK_DUMP_DIR")
                            .map(std::path::PathBuf::from)
                            .unwrap_or_else(|| {
                                std::path::PathBuf::from("/tmp/fn64-gfx-task-dumps")
                            });
                        std::fs::create_dir_all(&directory).unwrap_or_else(|error| {
                            panic!("failed to create FN64_GFX_TASK_DUMP_DIR {directory:?}: {error}")
                        });
                        let command_trace = gbi::trace_display_list_f3dex2(&*rdram, task.data_ptr);
                        let report = format!(
                            "task_index={dump_index}\noutput_addr={output_addr:#010x}\n\
                             reference_triangle_count={tri_count}\ntask={task:#?}\n{command_trace}",
                        );
                        let path = directory.join(format!("task-{dump_index:04}.txt"));
                        std::fs::write(&path, report).unwrap_or_else(|error| {
                            panic!("failed to write gfx task diagnostic {path:?}: {error}")
                        });
                        eprintln!(
                            "[fn64-render-reference] dumped gfx task #{dump_index} ({tri_count} reference \
                             triangles) to {path:?}"
                        );
                    }
                }
            }
        }

        let mut active_target = persistent_target;
        if self.decode_mode == DecodeMode::Simple && active_target.is_none() && output_addr != 0 {
            active_target = Some(gbi::ColorImage {
                format: gbi::ColorImage::RGBA_FORMAT,
                size: gbi::ColorImage::BITS_16,
                width: u16::try_from(fb_width).expect("reference framebuffer width exceeds u16"),
                address: output_addr,
            });
        }
        let target_loaded = persistent_target.is_some();
        {
            let fb = self.fb.as_mut().expect("framebuffer checked above");
            if self.decode_mode != DecodeMode::Simple {
                if let Some(target) = active_target {
                    validate_reference_color_image(rdram, fb_height, target)?;
                    load_color_image(rdram, target, fb, &mut self.rdram_hidden_bits);
                }
            }
            if let Some(target) = persistent_depth_image {
                load_rdp_depth_image(rdram, target, fb, &mut self.rdram_hidden_bits)?;
            }
        }

        Ok(PreparedReferenceTask::Ready(ReferenceTaskContinuation {
            token: self.allocate_continuation_token(),
            task: *task,
            output_addr,
            decode_mode: self.decode_mode,
            operations,
            next_operation: 0,
            active_target,
            target_loaded,
            active_depth_image: persistent_depth_image,
            active_primitive_depth: self.primitive_depth,
            saw_explicit_target: false,
            dirty: false,
            depth_dirty: false,
            reached_dp_full_sync: false,
            tri_count,
            persistent_target_was_selected: persistent_target.is_some(),
        }))
    }

    pub(super) fn process_reference_task_chunk(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
        step: fn64_render::RenderTaskStep,
    ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
        let state = match step {
            fn64_render::RenderTaskStep::Start => {
                match self.prepare_reference_task(rdram, rsp_memory, task, output_addr)? {
                    PreparedReferenceTask::Ready(state) => state,
                    PreparedReferenceTask::NeedsLle(ucode_sha256) => {
                        return Ok(fn64_render::RenderTaskChunkStatus::NeedsLle { ucode_sha256 });
                    }
                }
            }
            fn64_render::RenderTaskStep::Resume(token) => {
                let pending = self
                    .continuation
                    .as_ref()
                    .ok_or_else(|| RenderError::Backend {
                        backend: "reference-task-continuation",
                        reason: format!(
                            "continuation token {} is stale or was already consumed",
                            token.get()
                        ),
                    })?;
                if pending.token != token {
                    return Err(RenderError::Backend {
                        backend: "reference-task-continuation",
                        reason: format!(
                            "continuation token {} does not own retained token {}",
                            token.get(),
                            pending.token.get()
                        ),
                    });
                }
                if pending.task != *task || pending.output_addr != output_addr {
                    return Err(RenderError::Backend {
                        backend: "reference-task-continuation",
                        reason: format!(
                            "continuation token {} was resumed with a different task or output target",
                            token.get()
                        ),
                    });
                }
                // Interleaving closed here: chunk N has committed and token T
                // is visible to the scheduler; SIG0 may suspend T before a
                // later host boundary resumes it. Removing T before executing
                // operation N+1 means a duplicate/stale resume can never replay
                // that operation after its first successful consumption.
                let mut state = self
                    .continuation
                    .take()
                    .expect("validated reference continuation disappeared");
                state.token = self.allocate_continuation_token();
                state
            }
        };
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
        self.advance_reference_task_chunk(rdram, state)
    }

    pub(super) fn advance_reference_task_chunk(
        &mut self,
        rdram: &mut [u8],
        mut state: ReferenceTaskContinuation,
    ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
        if state.next_operation < state.operations.len() {
            let operation = state.operations[state.next_operation].clone();
            state.next_operation += 1;
            self.execute_reference_operation(rdram, &mut state, &operation)?;
            state.reached_dp_full_sync |= matches!(operation, gbi::RenderOp::FullSync);
            self.commit_reference_boundary(rdram, &state)?;
        }

        let dp_full_sync = if state.reached_dp_full_sync {
            fn64_render::DpFullSyncStatus::Reached
        } else {
            fn64_render::DpFullSyncStatus::NotReached
        };
        if state.next_operation < state.operations.len() {
            let token = state.token;
            assert!(
                self.continuation.replace(state).is_none(),
                "reference continuation ownership became occupied during one chunk"
            );
            self.last_dp_full_sync = dp_full_sync;
            Ok(fn64_render::RenderTaskChunkStatus::Continue(token))
        } else {
            self.finish_reference_task(rdram, state)?;
            self.last_dp_full_sync = dp_full_sync;
            Ok(fn64_render::RenderTaskChunkStatus::Complete)
        }
    }

    pub(super) fn execute_reference_operation(
        &mut self,
        rdram: &mut [u8],
        state: &mut ReferenceTaskContinuation,
        operation: &gbi::RenderOp,
    ) -> Result<(), RenderError> {
        let fb = self
            .fb
            .as_mut()
            .ok_or(RenderError::NotReady("create() not called"))?;
        #[cfg(not(test))]
        let no_depth = crate::debug_flag("FN64_NO_DEPTH");
        #[cfg(test)]
        let no_depth = false;

        match operation {
            gbi::RenderOp::Triangle(triangle) => {
                let disposition = require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    rdram.len(),
                    "F3DEX2 triangle",
                )?;
                if disposition == ColorTargetDisposition::DropWrites {
                    return Ok(());
                }
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "F3DEX2 triangle enables Z compare/update without a selected G_SETZIMG target"
                            .to_string(),
                    });
                }
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && triangle.other_mode.primitive_depth_source()
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "F3DEX2 triangle selects primitive Z without prior G_SETPRIMDEPTH"
                            .to_string(),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                if state.decode_mode == DecodeMode::Simple {
                    fb.draw_triangle(triangle);
                } else if no_depth {
                    fb.draw_triangle_no_depth_culled(triangle, triangle.cull);
                } else {
                    fb.draw_triangle_culled(triangle, triangle.cull);
                }
                state.depth_dirty |= !no_depth && triangle.other_mode.depth_update_enabled();
                state.dirty = true;
            }
            gbi::RenderOp::Line(line) => {
                if require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    rdram.len(),
                    "G_LINE3D",
                )? == ColorTargetDisposition::DropWrites
                {
                    return Ok(());
                }
                if !no_depth
                    && line.other_mode.depth_compare_enabled()
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "G_LINE3D enables Z compare without a selected G_SETZIMG target"
                            .to_string(),
                    });
                }
                if !no_depth
                    && line.other_mode.depth_compare_enabled()
                    && line.other_mode.primitive_depth_source()
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "G_LINE3D selects primitive Z without prior G_SETPRIMDEPTH"
                            .to_string(),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                if no_depth {
                    fb.draw_line_no_depth(line);
                } else {
                    fb.draw_line(line);
                }
                state.dirty = true;
            }
            gbi::RenderOp::RawTriangle(triangle) => {
                if require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    rdram.len(),
                    "raw RDP triangle",
                )? == ColorTargetDisposition::DropWrites
                {
                    return Ok(());
                }
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "raw RDP triangle enables Z compare/update without a selected G_SETZIMG target"
                            .to_string(),
                    });
                }
                if !no_depth
                    && (triangle.other_mode.depth_compare_enabled()
                        || triangle.other_mode.depth_update_enabled())
                    && ((triangle.other_mode.primitive_depth_source()
                        && state.active_primitive_depth.is_none())
                        || (!triangle.other_mode.primitive_depth_source() && triangle.z.is_none()))
                {
                    let reason = if triangle.other_mode.primitive_depth_source() {
                        "raw RDP triangle selects primitive Z without prior G_SETPRIMDEPTH"
                    } else {
                        "raw RDP triangle enables pixel Z compare/update without carrying Z coefficients"
                    };
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: reason.to_string(),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                if no_depth {
                    fb.draw_raw_rdp_triangle_no_depth(triangle);
                } else {
                    fb.draw_raw_rdp_triangle(triangle);
                }
                state.depth_dirty |= !no_depth && triangle.other_mode.depth_update_enabled();
                state.dirty = true;
            }
            gbi::RenderOp::SetColorImage(target) => {
                // G_SETCIMG is a LATCH, exactly like G_SETTIMG ("Pointer +
                // format latch only; no texel data moves until a G_LOAD*").
                // The RDP stores format/size/width/address and reads none of
                // it until a primitive writes through the target, so a
                // configuration this backend cannot execute is only an error
                // if something actually draws to it. WCW/nWo Revenge latches
                // format=0 size=0 and the eager check aborted the frame at
                // the latch.
                //
                // Deferring is not a relaxation: `require_reference_color_target`
                // already gates every drawing op, and each now validates the
                // latched target, so a draw through an unsupported format
                // still fails with this same message.
                // Loadable means BOTH a known layout and backing store. A
                // target above installed RDRAM has nothing to read in and
                // nothing to write out -- no Rambus device answers -- so it
                // latches but never loads.
                let supported =
                    target.layout().is_some() && !target.is_unbacked_rdram(rdram.len());
                let changes_target = state.active_target != Some(*target) || !state.target_loaded;
                if changes_target {
                    if let (Some(previous), Some(layout)) = (state.active_target, target.layout()) {
                        let transition = previous.transition_to(*target);
                        debug_assert_eq!(transition.to, layout);
                    }
                    if state.depth_dirty {
                        if let Some(depth_target) = state.active_depth_image {
                            commit_rdp_depth_image(
                                rdram,
                                depth_target,
                                fb,
                                &mut self.rdram_hidden_bits,
                            )?;
                        }
                        state.depth_dirty = false;
                    }
                    if state.dirty {
                        if let Some(previous) = state.active_target {
                            commit_color_image(rdram, previous, fb, &mut self.rdram_hidden_bits);
                        }
                    }
                    // Only a target with a known layout can be loaded into the
                    // framebuffer. An unsupported one is latched and left
                    // unloaded; the pending work for the PREVIOUS target was
                    // still committed above, so nothing already drawn is lost.
                    if supported {
                        validate_reference_color_image(rdram, fb.height, *target)?;
                        load_color_image(rdram, *target, fb, &mut self.rdram_hidden_bits);
                    }
                    if let Some(depth_target) = state.active_depth_image {
                        load_rdp_depth_image(rdram, depth_target, fb, &mut self.rdram_hidden_bits)?;
                    }
                    state.dirty = false;
                }
                state.active_target = Some(*target);
                state.target_loaded = supported;
                state.saw_explicit_target = true;
            }
            gbi::RenderOp::SetDepthImage(target) => {
                if state.active_depth_image != Some(*target) {
                    if state.depth_dirty {
                        if let Some(previous) = state.active_depth_image {
                            commit_rdp_depth_image(
                                rdram,
                                previous,
                                fb,
                                &mut self.rdram_hidden_bits,
                            )?;
                        }
                        state.depth_dirty = false;
                    }
                    load_rdp_depth_image(rdram, *target, fb, &mut self.rdram_hidden_bits)?;
                    state.active_depth_image = Some(*target);
                }
            }
            gbi::RenderOp::SetPrimitiveDepth(primitive_depth) => {
                state.active_primitive_depth = Some(*primitive_depth);
                fb.set_primitive_depth(state.active_primitive_depth);
            }
            gbi::RenderOp::FillRectangle(rectangle) => {
                if require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    rdram.len(),
                    "G_FILLRECT",
                )? == ColorTargetDisposition::DropWrites
                {
                    return Ok(());
                }
                validate_fill_rectangle(rectangle)?;
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason:
                            "combined G_FILLRECT selects primitive Z without prior G_SETPRIMDEPTH"
                                .into(),
                    });
                }
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: "combined G_FILLRECT enables depth without a G_SETZIMG target"
                            .into(),
                    });
                }
                let target = state.active_target.unwrap_or(gbi::ColorImage {
                    format: gbi::ColorImage::RGBA_FORMAT,
                    size: gbi::ColorImage::BITS_16,
                    width: u16::try_from(fb.width)
                        .expect("reference framebuffer width exceeds u16"),
                    address: 0,
                });
                fb.draw_fill_rectangle(rectangle, target);
                if rectangle.cycle_type == gbi::CycleType::Fill
                    && state.active_target.map(|target| target.address)
                        == state.active_depth_image.map(|target| target.address)
                {
                    fb.clear_depth_rectangle(rectangle);
                    state.depth_dirty = true;
                } else if rectangle.other_mode.depth_update_enabled() {
                    state.depth_dirty = true;
                }
                state.dirty = true;
            }
            gbi::RenderOp::TextureRectangle(rectangle) => {
                if require_reference_color_target(
                    state.decode_mode,
                    state.active_target,
                    rdram.len(),
                    texture_rectangle_name(rectangle),
                )? == ColorTargetDisposition::DropWrites
                {
                    return Ok(());
                }
                validate_texture_rectangle(rectangle, state.active_target)?;
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_primitive_depth.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: format!(
                            "{} selects primitive Z without prior G_SETPRIMDEPTH",
                            texture_rectangle_name(rectangle)
                        ),
                    });
                }
                if (rectangle.other_mode.depth_compare_enabled()
                    || rectangle.other_mode.depth_update_enabled())
                    && state.active_depth_image.is_none()
                {
                    return Err(RenderError::Backend {
                        backend: "reference",
                        reason: format!(
                            "{} enables Z compare/update without a selected G_SETZIMG target",
                            texture_rectangle_name(rectangle)
                        ),
                    });
                }
                fb.set_primitive_depth(state.active_primitive_depth);
                match rectangle.other_mode.cycle_type() {
                    gbi::CycleType::Copy => fb.draw_copy_texture_rectangle(rectangle),
                    gbi::CycleType::OneCycle | gbi::CycleType::TwoCycle => {
                        fb.draw_texture_rectangle(rectangle)
                    }
                    // "In FILL mode this behaves identically to Fill
                    // Rectangle, the texturing properties are ignored."
                    // Executing it as the Fill Rectangle it is documented to
                    // be reuses the existing fill-cycle rasterizer, so the
                    // inclusive-edge and no-subpixel rules come from one
                    // implementation rather than a second copy that could
                    // drift from it.
                    gbi::CycleType::Fill => {
                        let target = state.active_target.unwrap_or(gbi::ColorImage {
                            format: gbi::ColorImage::RGBA_FORMAT,
                            size: gbi::ColorImage::BITS_16,
                            width: u16::try_from(fb.width)
                                .expect("reference framebuffer width exceeds u16"),
                            address: 0,
                        });
                        fb.draw_fill_rectangle(&rectangle.as_fill_cycle_rectangle(), target);
                    }
                }
                state.depth_dirty |= rectangle.other_mode.depth_update_enabled();
                state.dirty = true;
            }
            gbi::RenderOp::FullSync => {
                if state.dirty {
                    // `dirty` is only set by a drawing op, and every drawing
                    // op rejects an unsupported latched target before it can
                    // set it -- so a dirty framebuffer always has a layout.
                    // Filtering rather than unwrapping keeps that an
                    // invariant rather than a panic if it ever stops holding.
                    if let Some(target) =
                        state.active_target.filter(|t| t.layout().is_some() && !t.is_unbacked_rdram(rdram.len()))
                    {
                        commit_color_image(rdram, target, fb, &mut self.rdram_hidden_bits);
                    }
                    state.dirty = false;
                }
                if state.depth_dirty {
                    if let Some(target) = state.active_depth_image {
                        commit_rdp_depth_image(rdram, target, fb, &mut self.rdram_hidden_bits)?;
                    }
                    state.depth_dirty = false;
                }
            }
        }
        Ok(())
    }

    pub(super) fn commit_reference_boundary(
        &mut self,
        rdram: &mut [u8],
        state: &ReferenceTaskContinuation,
    ) -> Result<(), RenderError> {
        let fb = self
            .fb
            .as_ref()
            .ok_or(RenderError::NotReady("create() not called"))?;
        if state.dirty {
            // See the FullSync commit: a dirty framebuffer always has a
            // supported layout, because drawing through an unsupported
            // latched target is rejected before it can dirty anything.
            if let Some(target) = state.active_target.filter(|t| t.layout().is_some() && !t.is_unbacked_rdram(rdram.len())) {
                commit_color_image(rdram, target, fb, &mut self.rdram_hidden_bits);
            }
        }
        if state.depth_dirty {
            if let Some(target) = state.active_depth_image {
                commit_rdp_depth_image(rdram, target, fb, &mut self.rdram_hidden_bits)?;
            }
        }
        Ok(())
    }

    pub(super) fn finish_reference_task(
        &mut self,
        rdram: &mut [u8],
        state: ReferenceTaskContinuation,
    ) -> Result<(), RenderError> {
        self.commit_reference_boundary(rdram, &state)?;
        if state.saw_explicit_target || state.persistent_target_was_selected {
            self.color_image = state.active_target;
        }
        self.depth_image = state.active_depth_image;
        self.primitive_depth = state.active_primitive_depth;

        #[cfg(not(test))]
        if matches!(state.decode_mode, DecodeMode::F3dex2 | DecodeMode::S2dex) {
            raster::zstat::summary();
        }

        if let Some(dump) = self.auto_dump.as_mut() {
            let fb = self
                .fb
                .as_ref()
                .ok_or(RenderError::NotReady("create() not called"))?;
            let idx = dump.task_index;
            dump.task_index += 1;
            if idx >= dump.skip_before_task {
                let [cr, cg, cb, ca] = self.clear_color;
                let non_clear = fb.has_non_uniform_content(cr, cg, cb, ca);
                if !non_clear {
                    eprintln!(
                        "[fn64-render-reference] gfx task #{idx}: decoded {} triangle(s); \
                         framebuffer is UNIFORM clear -- reported blank, not dumped.",
                        state.tri_count
                    );
                } else if dump.written >= dump.limit {
                    if !dump.limit_reported {
                        eprintln!(
                            "[fn64-render-reference] gfx task #{idx}: non-clear ({} tris) but \
                             auto-dump limit ({}) reached -- suppressing later dump notices.",
                            state.tri_count, dump.limit
                        );
                        dump.limit_reported = true;
                    }
                } else {
                    let _ = std::fs::create_dir_all(&dump.dir);
                    let path = dump
                        .dir
                        .join(format!("{}-{:04}.png", dump.prefix, dump.written));
                    match png_dump::write_png(&path, fb.width, fb.height, &fb.pixels) {
                        Ok(()) => {
                            dump.written += 1;
                            eprintln!(
                                "[fn64-render-reference] gfx task #{idx}: NON-CLEAR ({} tris) \
                                 -- dumped {}",
                                state.tri_count,
                                path.display()
                            );
                        }
                        Err(error) => eprintln!(
                            "[fn64-render-reference] gfx task #{idx}: failed to write {}: {error}",
                            path.display()
                        ),
                    }
                }
            }
        }
        Ok(())
    }
}
