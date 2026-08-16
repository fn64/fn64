//! The owning Context handle: constructor, per-frame calls, teardown.
use super::*;

pub(super) fn error_string(buffer: &[c_char; ERROR_CAPACITY], fallback: &str) -> String {
    // SAFETY: every C ABI operation receives the full buffer capacity and
    // the shim always writes a trailing NUL when it reports an error. The
    // zero-initialized Rust buffer also guarantees a NUL if no text arrived.
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if message.is_empty() {
        fallback.to_string()
    } else {
        message
    }
}

pub(crate) struct Context(NonNull<RawContext>);

pub(crate) struct PresentedPixelMetadata {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) row_bytes: u32,
    pub(crate) format: crate::Rt64PresentPixelFormat,
    pub(crate) graphics_api: ActiveRenderGraphicsApi,
    pub(crate) present_id: u64,
    pub(crate) workload_id: u64,
}

impl Context {
    pub(crate) fn create(
        width: u32,
        height: u32,
        nominal_refresh_rate: u32,
        user_settings: &RenderRuntimeSettings,
        enhancement_settings: &RenderEnhancementSettings,
        emulator_settings: &RenderEmulatorSettings,
    ) -> Result<Self, String> {
        let raw_user = RawUserConfig::from(user_settings);
        let raw_enhancement = RawEnhancementConfig::from(enhancement_settings);
        let raw_emulator = RawEmulatorConfig::from(emulator_settings);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: `error` is writable for the advertised capacity; the C++
        // shim returns either a uniquely-owned opaque context or null.
        let raw = unsafe {
            fn64_rt64_create(
                width,
                height,
                nominal_refresh_rate,
                &raw_user,
                &raw_enhancement,
                &raw_emulator,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        NonNull::new(raw)
            .map(Self)
            .ok_or_else(|| error_string(&error, "RT64 create failed without a diagnostic"))
    }

    /// Concrete API selected by the live RT64 application during successful
    /// setup. This queries device identity only; it neither submits work nor
    /// touches the present-capture or VI paths.
    pub(crate) fn live_device_graphics_api(&self) -> Result<ActiveRenderGraphicsApi, String> {
        let mut graphics_api = 0;
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context remains alive for this synchronous,
        // read-only query, and both output buffers are writable for their
        // advertised sizes. The C++ boundary retains no pointers.
        let queried = unsafe {
            fn64_rt64_read_live_device_graphics_api(
                self.0.as_ptr(),
                &mut graphics_api,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if queried == 0 {
            return Err(error_string(
                &error,
                "RT64 live-device graphics API query failed without a diagnostic",
            ));
        }
        active_graphics_api_from_tag(graphics_api, "live-device")
    }

    pub(crate) fn apply_user_config(
        &mut self,
        settings: &RenderRuntimeSettings,
    ) -> Result<bool, String> {
        let raw_settings = RawUserConfig::from(settings);
        let mut framebuffers_discarded = 0;
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The settings
        // and result pointers remain live for this synchronous call.
        let ok = unsafe {
            fn64_rt64_apply_user_config(
                self.0.as_ptr(),
                &raw_settings,
                &mut framebuffers_discarded,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 settings apply failed without a diagnostic",
            ))
        } else {
            match framebuffers_discarded {
                0 => Ok(false),
                1 => Ok(true),
                value => Err(format!(
                    "RT64 returned invalid framebuffer-discard boolean {value}"
                )),
            }
        }
    }

    pub(crate) fn apply_enhancement_config(
        &mut self,
        settings: &RenderEnhancementSettings,
    ) -> Result<(), String> {
        let raw_settings = RawEnhancementConfig::from(settings);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The settings
        // pointer remains live for this synchronous call.
        let ok = unsafe {
            fn64_rt64_apply_enhancement_config(
                self.0.as_ptr(),
                &raw_settings,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 enhancement apply failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn apply_emulator_config(
        &mut self,
        settings: &RenderEmulatorSettings,
    ) -> Result<(), String> {
        let raw_settings = RawEmulatorConfig::from(settings);
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The settings
        // pointer remains live for this synchronous call.
        let ok = unsafe {
            fn64_rt64_apply_emulator_config(
                self.0.as_ptr(),
                &raw_settings,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 emulator apply failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    fn apply_replacement_packs(
        &mut self,
        packs: &[(CString, RenderReplacementPackIdentity)],
        enabled: bool,
        reload: bool,
    ) -> Result<(), String> {
        let raw: Vec<_> = packs
            .iter()
            .map(|(path, identity)| RawReplacementPack {
                path_utf8: path.as_ptr(),
                expected_database: RawReplacementDatabaseConfig::from(identity),
            })
            .collect();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is uniquely borrowed; every CString and raw
        // entry remains live for this synchronous call and no pointer is kept.
        let function = if reload {
            fn64_rt64_reload_replacement_packs
        } else {
            fn64_rt64_load_replacement_packs
        };
        let ok = unsafe {
            function(
                self.0.as_ptr(),
                raw.as_ptr(),
                raw.len(),
                u32::from(enabled),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 replacement-pack apply failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn load_replacement_packs(
        &mut self,
        packs: &[(CString, RenderReplacementPackIdentity)],
        enabled: bool,
    ) -> Result<(), String> {
        self.apply_replacement_packs(packs, enabled, false)
    }

    pub(crate) fn reload_replacement_packs(
        &mut self,
        packs: &[(CString, RenderReplacementPackIdentity)],
        enabled: bool,
    ) -> Result<(), String> {
        self.apply_replacement_packs(packs, enabled, true)
    }

    pub(crate) fn set_replacement_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed for this scalar
        // synchronous call.
        let ok = unsafe {
            fn64_rt64_set_replacement_enabled(
                self.0.as_ptr(),
                u32::from(enabled),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 replacement enable failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn wait_texture_replacement_state(
        &mut self,
        texture_hash: Option<u64>,
        require_replacement: bool,
    ) -> Result<crate::Rt64TextureReplacementEvidence, String> {
        let mut raw = RawTextureReplacementState::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; the state and
        // diagnostic buffers remain writable for this synchronous wait.
        let ok = unsafe {
            fn64_rt64_wait_texture_replacement_state(
                self.0.as_ptr(),
                texture_hash.unwrap_or(0),
                u32::from(require_replacement),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 texture-replacement evidence failed without a diagnostic",
            ));
        }
        Self::texture_replacement_evidence_from_raw(raw)
    }

    fn texture_replacement_evidence_from_raw(
        raw: RawTextureReplacementState,
    ) -> Result<crate::Rt64TextureReplacementEvidence, String> {
        let boolean = |name: &str, value: u32| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("RT64 returned invalid {name} boolean {value}")),
        };
        Ok(crate::Rt64TextureReplacementEvidence {
            texture_hash: raw.texture_hash,
            stream_load_count: raw.stream_load_count,
            texture_count: raw.texture_count,
            texture_known: boolean("texture-known", raw.texture_known)?,
            replacement_resolved: boolean("replacement-resolved", raw.replacement_resolved)?,
            replacement_installed: boolean("replacement-installed", raw.replacement_installed)?,
            replacement_mip_levels: raw.replacement_mip_levels,
            replacements_enabled: boolean("replacements-enabled", raw.replacements_enabled)?,
            stream_queued: raw.stream_queued,
            stream_active: raw.stream_active,
            stream_results_pending: raw.stream_results_pending,
            uploads_pending: raw.uploads_pending,
            resolved_paths_pending: raw.resolved_paths_pending,
            observed_resolved_not_installed: boolean(
                "observed-resolved-not-installed",
                raw.observed_resolved_not_installed,
            )?,
            stream_workers_paused: boolean("stream-workers-paused", raw.stream_workers_paused)?,
            stream_worker_count: raw.stream_worker_count,
        })
    }

    pub(crate) fn set_stream_workers_paused(&mut self, paused: bool) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. The strict C++
        // control accepts only a quiescent worker set and retains no pointer.
        let ok = unsafe {
            fn64_rt64_set_stream_workers_paused(
                self.0.as_ptr(),
                u32::from(paused),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(
                &error,
                "RT64 stream-worker evidence control failed without a diagnostic",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn wait_stream_fallback_state(
        &mut self,
        texture_hash: u64,
    ) -> Result<crate::Rt64TextureReplacementEvidence, String> {
        let mut raw = RawTextureReplacementState::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; output and
        // diagnostic buffers remain writable for the synchronous state wait.
        let ok = unsafe {
            fn64_rt64_wait_stream_fallback_state(
                self.0.as_ptr(),
                texture_hash,
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 stream-fallback evidence failed without a diagnostic",
            ));
        }
        Self::texture_replacement_evidence_from_raw(raw)
    }

    pub(crate) fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut RspMemory,
        task: &OsTask,
        output_addr: u32,
        admission: &crate::Rt64TaskAdmission,
    ) -> Result<NativeTaskOutcome, String> {
        let raw_task = RawTask::from(task);
        let prepared_plan = PreparedUcodePlan::new(admission)?;
        let raw_plan = prepared_plan.raw();
        let mut dmem = *rsp_memory.bank(RspMemoryBank::Dmem);
        let mut imem = *rsp_memory.bank(RspMemoryBank::Imem);
        let mut result = RawTaskResult::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed; both
        // slice pointer/length and the repr(C) task remain valid for the
        // synchronous call. The shim waits for RT64's render-to-RAM worker
        // before returning, so no foreign thread retains the Rust borrow.
        let ok = unsafe {
            fn64_rt64_process_task(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                dmem.as_mut_ptr(),
                dmem.len(),
                imem.as_mut_ptr(),
                imem.len(),
                &raw_task,
                output_addr,
                &raw_plan,
                &mut result,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            let expected_generation_count = u32::try_from(prepared_plan.generations.len())
                .expect("validated microcode generation count fits u32");
            let outcome =
                task_result_from_raw(result, expected_generation_count, prepared_plan.plan_sha256)?;
            if matches!(outcome, NativeTaskOutcome::Complete(_)) {
                if rsp_memory.bank(RspMemoryBank::Dmem) != &dmem {
                    rsp_memory
                        .write_bytes(RspMemAddr::from_register(0), &dmem)
                        .expect("RT64 returned a complete 4 KiB DMEM bank");
                }
                if rsp_memory.bank(RspMemoryBank::Imem) != &imem {
                    rsp_memory
                        .write_bytes(RspMemAddr::from_register(0x1000), &imem)
                        .expect("RT64 returned a complete 4 KiB IMEM bank");
                }
            }
            Ok(outcome)
        } else {
            Err(error_string(
                &error,
                "RT64 task processing failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
    ) -> Result<(), String> {
        // Waits for completion -- the safe, unconditional default every
        // existing caller keeps getting. Use `process_rdp_commands_async`
        // when the caller itself will wait before it next needs completed
        // GPU state (present, or a later submission in the same field
        // passing `wait_for_completion = true`).
        self.process_rdp_commands_inner(rdram, start, end, output_addr, true)
    }

    /// Submit without blocking for GPU completion.
    ///
    /// `waitForWorkloadId` compares a monotonic counter (`waitId <=
    /// workloadId`), so RT64's queue is strictly FIFO: waiting for
    /// submission N's id also waits for every submission before it. A field
    /// with several coalesced DPC ranges therefore only needs ONE wait, on
    /// the last one -- not one after each. Measured on the render-benchmark
    /// route (rt64 lane): `waitForWorkloadId` inside this call was the
    /// majority of an ~11 ms/field cost paid up to ~2.9 times/field, when
    /// only the final submission before the frame is consumed needs to
    /// block at all.
    ///
    /// SAFETY of skipping the wait: nothing between this call and the
    /// caller's own next wait may read completed-workload state. The C++
    /// side already enforces this for its own internal readers (deferred
    /// capture, present capture force the wait regardless of this flag);
    /// the Rust caller's obligation is to pass `wait_for_completion = true`
    /// on the LAST submission of a field, so the field's own completion is
    /// still established before anything downstream reads the framebuffer.
    pub(crate) fn process_rdp_commands_async(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
        wait_for_completion: bool,
    ) -> Result<(), String> {
        self.process_rdp_commands_inner(rdram, start, end, output_addr, wait_for_completion)
    }

    fn process_rdp_commands_inner(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
        wait_for_completion: bool,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. RT64 waits for
        // the submitted render-to-RAM workload before this call returns IFF
        // `wait_for_completion` is nonzero; see `process_rdp_commands_async`
        // for the caller obligation when it is not.
        let ok = unsafe {
            fn64_rt64_process_rdp_commands(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                start,
                end,
                output_addr,
                c_int::from(wait_for_completion),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 raw RDP processing failed without a diagnostic",
            ))
        }
    }

    /// Wait for whatever workload is currently outstanding.
    ///
    /// `process_rdp_commands_async(.., wait_for_completion: false)` can leave
    /// GPU work in flight. Anything about to read completed-frame state (a
    /// present, most obviously) must flush first. Cheap when nothing is
    /// outstanding: `waitForWorkloadId` against an already-reached id returns
    /// immediately (rt64_workload_queue.cpp:93-95, `waitId <= workloadId`).
    pub(crate) fn flush_pending_workload(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_flush_pending_workload(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 workload flush failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn present(
        &mut self,
        memory: &fn64_runtime::PhysicalRdramRead<'_>,
        vi: ViPresentation,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        validate_native_vi_filters(&vi)?;
        let vi = raw_vi(vi)?;
        // SAFETY: the opaque context is alive and uniquely borrowed. The
        // call-scoped physical capability proves the exact 8 MiB allocation
        // remains live. This entry only reads VI source bytes; the shim waits
        // every present worker and restores its placeholder aliases before
        // returning.
        let ok = unsafe {
            fn64_rt64_present(
                self.0.as_ptr(),
                memory.as_mut_ptr(),
                memory.len(),
                &vi,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 present failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn enable_present_capture(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed. Hook
        // registration is synchronous and retains no Rust-owned pointer.
        let ok = unsafe {
            fn64_rt64_enable_present_capture(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 present capture could not be enabled without a diagnostic",
            ))
        }
    }

    /// Concrete graphics API observed from the most recent completed capture.
    /// The C++ hook publishes this under the same mutex and generation as the
    /// pixel geometry, so requested settings cannot manufacture the value.
    pub(crate) fn presented_graphics_api(&self) -> Result<ActiveRenderGraphicsApi, String> {
        let mut metadata = RawPresentCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive. The C++ metadata-only query locks the
        // capture owner and writes only the caller-owned output/error buffers.
        let queried = unsafe {
            fn64_rt64_read_present_capture(
                self.0.as_ptr(),
                &mut metadata,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if queried == 0 {
            return Err(error_string(
                &error,
                "RT64 present capture query failed without a diagnostic",
            ));
        }
        let (_, _, graphics_api) = validate_present_capture_metadata(metadata)?;
        Ok(graphics_api)
    }

    pub(crate) fn presented_pixels_into(
        &mut self,
        reuse: &mut Vec<u8>,
    ) -> Result<crate::Rt64PresentedPixels, String> {
        let metadata = self.read_presented_pixels_into(reuse)?;
        Ok(crate::Rt64PresentedPixels {
            width: metadata.width,
            height: metadata.height,
            row_bytes: metadata.row_bytes,
            format: metadata.format,
            graphics_api: metadata.graphics_api,
            present_id: metadata.present_id,
            workload_id: metadata.workload_id,
            bytes: std::mem::take(reuse),
        })
    }

    pub(crate) fn read_presented_pixels_into(
        &mut self,
        bytes: &mut Vec<u8>,
    ) -> Result<PresentedPixelMetadata, String> {
        let mut metadata = RawPresentCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; null with zero
        // capacity is the C API's metadata-only query form.
        let queried = unsafe {
            fn64_rt64_read_present_capture(
                self.0.as_ptr(),
                &mut metadata,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if queried == 0 {
            return Err(error_string(
                &error,
                "RT64 present capture query failed without a diagnostic",
            ));
        }
        let (byte_len, format, graphics_api) = validate_present_capture_metadata(metadata)?;
        bytes.resize(byte_len, 0);
        let queried_metadata = metadata;
        error.fill(0);
        // SAFETY: `bytes` is writable for exactly the capacity advertised by
        // the preceding metadata query. No later present can race this call
        // through the unique Rust borrow of the context.
        let read = unsafe {
            fn64_rt64_read_present_capture(
                self.0.as_ptr(),
                &mut metadata,
                bytes.as_mut_ptr(),
                bytes.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if read == 0 {
            return Err(error_string(
                &error,
                "RT64 present capture read failed without a diagnostic",
            ));
        }
        if metadata != queried_metadata {
            return Err("RT64 present capture metadata changed during synchronous readback".into());
        }
        Ok(PresentedPixelMetadata {
            width: metadata.width,
            height: metadata.height,
            row_bytes: metadata.row_bytes,
            format,
            graphics_api,
            present_id: metadata.present_id,
            workload_id: metadata.workload_id,
        })
    }

    pub(crate) fn present_selection(&mut self) -> Result<crate::Rt64PresentSelection, String> {
        let mut selection = RawPresentSelection::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed. The C++
        // query waits both RT64 queue workers idle before reading descriptor
        // and render-target state into this fixed-size output value.
        let ok = unsafe {
            fn64_rt64_read_present_selection(
                self.0.as_ptr(),
                &mut selection,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 present-selection query failed without a diagnostic",
            ));
        }
        if selection.reserved != 0 {
            return Err("RT64 present selection returned nonzero reserved metadata".into());
        }
        Ok(crate::Rt64PresentSelection {
            present_id: selection.present_id,
            source_texture_identity: selection.source_texture_identity,
            target_address: selection.target_address,
            target_width: selection.target_width,
            target_height: selection.target_height,
            target_size: selection.target_size,
            workload_resolution_scale_x: selection.workload_resolution_scale_x,
            workload_resolution_scale_y: selection.workload_resolution_scale_y,
            resolution_scale_x: selection.resolution_scale_x,
            resolution_scale_y: selection.resolution_scale_y,
            raster_width: selection.raster_width,
            raster_height: selection.raster_height,
            downsample_multiplier: selection.downsample_multiplier,
        })
    }

    pub(crate) fn enable_deferred_workload_capture(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. Arming only
        // changes shim-owned evidence state after both RT64 workers are idle.
        let ok = unsafe {
            fn64_rt64_enable_deferred_workload_capture(
                self.0.as_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 deferred-workload capture could not be armed without a diagnostic",
            ))
        }
    }

    pub(crate) fn deferred_workload_evidence(
        &mut self,
    ) -> Result<crate::Rt64DeferredWorkloadEvidence, String> {
        let mut raw = RawDeferredWorkloadEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; C++ waits both
        // worker queues idle before copying fixed-size scalar snapshots.
        let ok = unsafe {
            fn64_rt64_read_deferred_workload_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 deferred-workload evidence query failed without a diagnostic",
            ));
        }
        Ok(crate::Rt64DeferredWorkloadEvidence {
            pre_submission: deferred_snapshot(raw.pre_submission),
            current: deferred_snapshot(raw.current),
        })
    }

    pub(crate) fn framebuffer_copy_path_evidence(
        &mut self,
    ) -> Result<crate::Rt64FramebufferCopyPathEvidence, String> {
        let mut raw = RawFramebufferCopyPathEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed; C++ waits both
        // worker queues idle before reading the completed workload's bounded
        // path counters into this fixed-size value.
        let ok = unsafe {
            fn64_rt64_read_framebuffer_copy_path_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 framebuffer-copy path evidence query failed without a diagnostic",
            ));
        }
        if raw.reserved != 0 {
            return Err("RT64 returned nonzero reserved framebuffer-copy evidence".into());
        }
        let path = match raw.path {
            FRAMEBUFFER_COPY_PATH_GPU => crate::Rt64FramebufferCopyPath::GpuTileCopy,
            FRAMEBUFFER_COPY_PATH_CPU => crate::Rt64FramebufferCopyPath::CpuRdramTmemUpload,
            other => {
                return Err(format!(
                    "RT64 returned unknown framebuffer-copy path tag {other}"
                ));
            }
        };
        Ok(crate::Rt64FramebufferCopyPathEvidence {
            workload_id: raw.workload_id,
            source_framebuffer_identity: raw.source_framebuffer_identity,
            source_framebuffer_address: raw.source_framebuffer_address,
            path,
            gpu_create_tile_copy_operation_count: raw.gpu_create_tile_copy_operation_count,
            gpu_tile_dispatch_count: raw.gpu_tile_dispatch_count,
            cpu_rdram_tmem_upload_count: raw.cpu_rdram_tmem_upload_count,
            raw_tmem_tile_count: raw.raw_tmem_tile_count,
            sync_framebuffer_pair_count: raw.sync_framebuffer_pair_count,
        })
    }

    pub(crate) fn s2dex_fast_path_evidence(
        &mut self,
    ) -> Result<crate::Rt64S2dexFastPathEvidence, String> {
        let mut raw = RawS2dexFastPathEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: both renderer queues are joined by C++ before it copies the
        // completed workload's scalar/vector counts into this fixed wire image.
        let ok = unsafe {
            fn64_rt64_read_s2dex_fast_path_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 S2DEX fast-path evidence query failed without a diagnostic",
            ));
        }
        if raw.reserved != 0 || raw.source_is_managed_framebuffer > 1 {
            return Err("RT64 returned invalid S2DEX fast-path evidence wire fields".into());
        }
        Ok(crate::Rt64S2dexFastPathEvidence {
            workload_id: raw.workload_id,
            source_framebuffer_identity: raw.source_framebuffer_identity,
            load_operation_digest: raw.load_operation_digest,
            source_address: raw.source_address,
            source_width: raw.source_width,
            source_height: raw.source_height,
            source_size: raw.source_size,
            gpu_create_tile_copy_operation_count: raw.gpu_create_tile_copy_operation_count,
            gpu_tile_dispatch_count: raw.gpu_tile_dispatch_count,
            cpu_rdram_tmem_upload_count: raw.cpu_rdram_tmem_upload_count,
            raw_tmem_tile_count: raw.raw_tmem_tile_count,
            sync_framebuffer_pair_count: raw.sync_framebuffer_pair_count,
            framebuffer_pair_count: raw.framebuffer_pair_count,
            valid_tile_count: raw.valid_tile_count,
            load_operation_count: raw.load_operation_count,
            distinct_source_address_count: raw.distinct_source_address_count,
            minimum_source_address: raw.minimum_source_address,
            maximum_source_address: raw.maximum_source_address,
            base_source_load_count: raw.base_source_load_count,
            offset_source_load_count: raw.offset_source_load_count,
            source_is_managed_framebuffer: raw.source_is_managed_framebuffer != 0,
        })
    }

    pub(crate) fn enable_extended_gbi_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits both
        // queues idle and arms only shim-owned pass-through observation state.
        let ok = unsafe {
            fn64_rt64_enable_extended_gbi_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 Extended-GBI evidence could not be armed without a diagnostic",
            ))
        }
    }

    pub(crate) fn extended_gbi_evidence(
        &mut self,
    ) -> Result<crate::Rt64ExtendedGbiEvidence, String> {
        let mut raw = RawExtendedGbiEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits both
        // RT64 queues idle before copying one fixed-size bounded wire image.
        let ok = unsafe {
            fn64_rt64_read_extended_gbi_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 Extended-GBI evidence query failed without a diagnostic",
            ));
        }
        extended_evidence_from_raw(raw)
    }

    pub(crate) fn extended_presented_pixels(
        &mut self,
    ) -> Result<Vec<crate::Rt64ExtendedPresentedPixels>, String> {
        let mut captures: Vec<crate::Rt64ExtendedPresentedPixels> = Vec::new();
        let mut expected_count = None;
        for index in 0..EXTENDED_MAX_GENERATED_PRESENTS {
            let mut metadata = RawExtendedPresentCapture::default();
            let mut error = [0; ERROR_CAPACITY];
            // SAFETY: metadata-only query with a null byte pointer and zero
            // capacity. Extended evidence finalization already joined the
            // present worker before exposing this retained slot.
            let queried = unsafe {
                fn64_rt64_read_extended_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    std::ptr::null_mut(),
                    0,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if queried == 0 {
                return Err(error_string(
                    &error,
                    "RT64 Extended present-capture query failed without a diagnostic",
                ));
            }
            let count = usize::try_from(metadata.capture_count)
                .map_err(|_| "RT64 Extended capture count exceeds host space".to_string())?;
            if count == 0 || count > EXTENDED_MAX_GENERATED_PRESENTS {
                return Err("RT64 Extended capture count exceeds bounded capacity".into());
            }
            if let Some(expected) = expected_count {
                if count != expected {
                    return Err("RT64 Extended capture count changed during readback".into());
                }
            } else {
                expected_count = Some(count);
            }
            let byte_len = usize::try_from(metadata.byte_len)
                .map_err(|_| "RT64 Extended capture exceeds host address space".to_string())?;
            let mut bytes = vec![0; byte_len];
            let queried_metadata = metadata;
            error.fill(0);
            // SAFETY: the byte allocation exactly matches the metadata-only
            // query and the unique context borrow excludes a concurrent Rust
            // producer while C++ retains the slot under its capture mutex.
            let read = unsafe {
                fn64_rt64_read_extended_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if read == 0 {
                return Err(error_string(
                    &error,
                    "RT64 Extended present-capture read failed without a diagnostic",
                ));
            }
            if metadata != queried_metadata {
                return Err("RT64 Extended capture metadata changed during readback".into());
            }
            if metadata.capture_ordinal != index as u32 {
                return Err("RT64 Extended capture ordinal changed during ordered readback".into());
            }
            let capture = extended_present_capture_from_raw(metadata, bytes)?;
            if let Some(first) = captures.first() {
                if capture.workload_id != first.workload_id
                    || capture.present_id != first.present_id
                    || capture.capture_generation <= captures.last().unwrap().capture_generation
                {
                    return Err(
                        "RT64 Extended capture history identity or generation order changed".into(),
                    );
                }
            }
            captures.push(capture);
            if captures.len() == count {
                return Ok(captures);
            }
        }
        Err("RT64 Extended capture count exceeds bounded capacity".into())
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn enable_hfr_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ joins both
        // workers before arming only shim-owned bounded capture state.
        let ok = unsafe {
            fn64_rt64_enable_hfr_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 HFR evidence could not be armed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "synthetic-f3dex2-evidence")]
    pub(crate) fn process_synthetic_hfr_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the mutable allocation is valid for the passed length and
        // uniquely borrowed for the synchronous evidence-only HLE call.
        let ok = unsafe {
            fn64_rt64_process_synthetic_hfr_f3dex2(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                display_list,
                output_addr,
                original_refresh_rate,
                std::ptr::null_mut(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "synthetic RT64 HFR F3DEX2 processing failed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "region-rate-evidence")]
    pub(crate) fn process_synthetic_region_rate_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<crate::Rt64RegionRateEvidence, String> {
        let mut raw = RawRegionRateEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the mutable allocation and evidence output are valid for
        // the synchronous evidence-only HLE call and uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_process_synthetic_hfr_f3dex2(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                display_list,
                output_addr,
                0,
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "synthetic RT64 region-rate F3DEX2 processing failed without a diagnostic",
            ));
        }
        if raw.workload_id == 0
            || raw.extended_refresh_override_absent != 1
            || raw.configured_nominal_refresh_rate != raw.registered_nominal_refresh_rate
        {
            return Err("RT64 region-rate evidence returned inconsistent authority".into());
        }
        Ok(crate::Rt64RegionRateEvidence {
            workload_id: raw.workload_id,
            configured_nominal_refresh_rate: raw.configured_nominal_refresh_rate,
            registered_nominal_refresh_rate: raw.registered_nominal_refresh_rate,
            workload_original_refresh_rate: raw.workload_original_refresh_rate,
        })
    }

    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub(crate) fn process_synthetic_s2dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), String> {
        self.process_synthetic_s2dex_wire(rdram, display_list, output_addr, false)
    }

    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub(crate) fn process_synthetic_s2dex_wire(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
        legacy_wire: bool,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the allocation is valid for its passed length and uniquely
        // borrowed throughout the synchronous evidence-only HLE call.
        let ok = unsafe {
            fn64_rt64_process_synthetic_s2dex2(
                self.0.as_ptr(),
                rdram.as_mut_ptr(),
                rdram.len(),
                display_list,
                output_addr,
                u32::from(legacy_wire),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "synthetic RT64 S2DEX2 processing failed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "extended-gbi-evidence")]
    pub(crate) fn process_synthetic_extended_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), String> {
        self.process_synthetic_hfr_f3dex2(rdram, display_list, output_addr, 60)
            .map_err(|reason| reason.replace("synthetic RT64 HFR", "synthetic RT64 Extended-GBI"))
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn hfr_evidence(&mut self) -> Result<crate::Rt64HfrEvidence, String> {
        let mut raw = RawHfrEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: C++ joins both workers and copies one fixed-size scalar wire
        // image while the live context is uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_read_hfr_evidence(self.0.as_ptr(), &mut raw, error.as_mut_ptr(), error.len())
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 HFR evidence query failed without a diagnostic",
            ));
        }
        hfr_evidence_from_raw(raw)
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn hfr_presented_pixels(
        &mut self,
    ) -> Result<Vec<crate::Rt64HfrPresentedPixels>, String> {
        let mut captures: Vec<crate::Rt64HfrPresentedPixels> = Vec::new();
        let mut expected_count = None;
        for index in 0..EXTENDED_MAX_GENERATED_PRESENTS {
            let mut metadata = RawExtendedPresentCapture::default();
            let mut error = [0; ERROR_CAPACITY];
            // SAFETY: null bytes with zero capacity is the metadata-only form;
            // the HFR evidence query finalized and joined this history.
            let queried = unsafe {
                fn64_rt64_read_hfr_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    std::ptr::null_mut(),
                    0,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if queried == 0 {
                return Err(error_string(
                    &error,
                    "RT64 HFR present-capture query failed without a diagnostic",
                ));
            }
            let count = usize::try_from(metadata.capture_count)
                .map_err(|_| "RT64 HFR capture count exceeds host space".to_string())?;
            if count == 0 || count > EXTENDED_MAX_GENERATED_PRESENTS {
                return Err("RT64 HFR capture count exceeds bounded capacity".into());
            }
            if let Some(expected) = expected_count {
                if count != expected {
                    return Err("RT64 HFR capture count changed during readback".into());
                }
            } else {
                expected_count = Some(count);
            }
            let byte_len = usize::try_from(metadata.byte_len)
                .map_err(|_| "RT64 HFR capture exceeds host address space".to_string())?;
            let mut bytes = vec![0; byte_len];
            let queried_metadata = metadata;
            error.fill(0);
            // SAFETY: the allocation exactly matches the preceding metadata
            // query, and the unique borrow excludes a new Rust producer.
            let read = unsafe {
                fn64_rt64_read_hfr_present_capture(
                    self.0.as_ptr(),
                    index as u32,
                    &mut metadata,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if read == 0 {
                return Err(error_string(
                    &error,
                    "RT64 HFR present-capture read failed without a diagnostic",
                ));
            }
            if metadata != queried_metadata || metadata.capture_ordinal != index as u32 {
                return Err("RT64 HFR capture metadata changed during ordered readback".into());
            }
            let capture = hfr_present_capture_from_raw(metadata, bytes)?;
            if let Some(first) = captures.first() {
                if capture.workload_id != first.workload_id
                    || capture.present_id != first.present_id
                    || capture.capture_generation <= captures.last().unwrap().capture_generation
                {
                    return Err("RT64 HFR capture identity or generation order changed".into());
                }
            }
            captures.push(capture);
            if captures.len() == count {
                return Ok(captures);
            }
        }
        Err("RT64 HFR capture count exceeds bounded capacity".into())
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn enable_hfr_pacing_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the live context is uniquely borrowed. C++ joins both queue
        // workers before resetting and arming mutex-protected bounded state.
        let ok = unsafe {
            fn64_rt64_enable_hfr_pacing_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 HFR pacing evidence could not be armed without a diagnostic",
            ))
        }
    }

    #[cfg(feature = "hfr-evidence")]
    pub(crate) fn hfr_pacing_evidence(&mut self) -> Result<crate::Rt64HfrPacingEvidence, String> {
        let mut raw = RawHfrPacingEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: C++ joins both workers before copying the fixed-size scalar
        // wire image while the live context is uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_read_hfr_pacing_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 HFR pacing evidence query failed without a diagnostic",
            ));
        }
        hfr_pacing_from_raw(raw)
    }

    pub(crate) fn set_debugger_inspection_for_evidence(
        &mut self,
        paused: bool,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: bool,
    ) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the live context is uniquely borrowed. C++ first waits both
        // RT64 queue threads idle, validates every selected index, then updates
        // the backend-independent DebuggerInspector state by scalar value.
        let ok = unsafe {
            fn64_rt64_set_debugger_inspection_for_evidence(
                self.0.as_ptr(),
                u32::from(paused),
                framebuffer_index,
                draw_call_index,
                u32::from(framebuffer_depth),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 debugger evidence control failed without a diagnostic",
            ))
        }
    }

    pub(crate) fn enable_ubershader_evidence(&mut self) -> Result<(), String> {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits both
        // queue workers idle, joins ubershader construction, then installs a
        // process-global Metal hook with exclusive ownership validation.
        let ok = unsafe {
            fn64_rt64_enable_ubershader_evidence(self.0.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(error_string(
                &error,
                "RT64 ubershader evidence could not be enabled without a diagnostic",
            ))
        }
    }

    pub(crate) fn ubershader_evidence(&mut self) -> Result<crate::Rt64UbershaderEvidence, String> {
        let mut raw = RawUbershaderEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the context is alive and uniquely borrowed. C++ waits the
        // workload and present workers idle before copying atomic counters and
        // bounded renderer-owned scalar evidence into this fixed-size value.
        let ok = unsafe {
            fn64_rt64_read_ubershader_evidence(
                self.0.as_ptr(),
                &mut raw,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(error_string(
                &error,
                "RT64 ubershader evidence query failed without a diagnostic",
            ));
        }
        Ok(crate::Rt64UbershaderEvidence {
            workload_id: raw.workload_id,
            present_id: raw.present_id,
            descriptor_digest: raw.descriptor_digest,
            pipeline_digest: raw.pipeline_digest,
            graphics_pipeline_construction_events: raw.graphics_pipeline_construction_events,
            background_construction_events: raw.background_construction_events,
            caller_construction_events: raw.caller_construction_events,
            workload_construction_events: raw.workload_construction_events,
            present_construction_events: raw.present_construction_events,
            precreated_pipeline_count: raw.precreated_pipeline_count,
            raster_call_count: raw.raster_call_count,
            matched_ubershader_call_count: raw.matched_ubershader_call_count,
            specialized_shader_count: raw.specialized_shader_count,
            ubershaders_only: raw.ubershaders_only != 0,
            shader_hashes: raw.shader_hashes,
            pipeline_state_indices: raw.pipeline_state_indices,
            pipeline_identities: raw.pipeline_identities,
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the opaque context is alive and uniquely borrowed.
        let ok = unsafe {
            fn64_rt64_resize(
                self.0.as_ptr(),
                width,
                height,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_ne!(
            ok,
            0,
            "{}",
            error_string(&error, "RT64 resize failed without a diagnostic")
        );
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: Context is the unique owner of the pointer returned by
        // fn64_rt64_create and calls destroy exactly once.
        unsafe { fn64_rt64_destroy(self.0.as_ptr()) };
    }
}
