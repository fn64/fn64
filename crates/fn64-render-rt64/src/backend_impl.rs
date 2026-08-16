//! Inherent construction, configuration, and capture methods of
//! [`Rt64Backend`], split from lib.rs. The RenderBackend trait impl
//! deliberately stays in lib.rs: two guard tests scan that file for the
//! decoder-path and rollback invariants of the trait region.
use super::*;

impl Rt64Backend {
    pub fn new() -> Self {
        Rt64Backend {
            active_tv_type: None,
            active_surface_size: None,
            f3dex2_ucodes: F3dex2UcodeCatalog::default(),
            microcode_pairs: MicrocodePairCatalog::default(),
            last_dp_full_sync: fn64_render::DpFullSyncStatus::Unidentified,
            #[cfg(feature = "rt64")]
            context: None,
            #[cfg(feature = "rt64")]
            native_rdram_preimage: Vec::new(),
            #[cfg(not(feature = "rt64"))]
            created: false,
            last_present: None,
            configured_settings: RenderRuntimeSettings::default(),
            active_settings: None,
            configured_enhancement_settings: RenderEnhancementSettings::default(),
            active_enhancement_settings: None,
            configured_emulator_settings: RenderEmulatorSettings::default(),
            active_emulator_settings: None,
            configured_replacement_packs: Vec::new(),
            configured_replacement_enabled: RenderReplacementSettings::default().enabled,
            active_replacement_settings: None,
            #[cfg(feature = "rt64")]
            active_replacement_snapshot: None,
        }
    }

    #[cfg(any(feature = "rt64", test))]
    pub(crate) fn clear_active_native_identity(&mut self) {
        self.active_tv_type = None;
        self.active_surface_size = None;
        self.last_present = None;
        self.active_settings = None;
        self.active_enhancement_settings = None;
        self.active_emulator_settings = None;
        self.active_replacement_settings = None;
        #[cfg(feature = "rt64")]
        {
            self.active_replacement_snapshot = None;
        }
        self.last_dp_full_sync = fn64_render::DpFullSyncStatus::Unidentified;
    }

    // `pub(super)`: the RenderBackend trait impl deliberately stayed in
    // lib.rs (two guard tests scan that file for its decoder-path and
    // rollback invariants), and four of its arms call this. A child module's
    // private item is not visible to its PARENT, so leaving this private
    // broke exactly those four call sites under the `rt64` feature.
    #[cfg(feature = "rt64")]
    pub(super) fn invalidate_native_state(&mut self) {
        self.context = None;
        self.clear_active_native_identity();
    }

    /// Present one complete live register image against a standalone
    /// embedder's exact physical allocation. Integrated execution reaches the
    /// same required trait seam through its raw, higher-ranked capability.
    pub fn present_live(&mut self, rdram: &[u8], vi: ViPresentation) -> Result<(), RenderError> {
        <Self as RenderBackend>::present(
            self,
            PresentRequest::live(vi, fn64_runtime::PhysicalRdramRead::from_storage(rdram)),
        )
    }

    /// Present with explicit synthesized backend geometry. This compatibility
    /// path can drive standalone behavior tests but cannot produce release
    /// evidence.
    pub fn present_physical_compatibility(
        &mut self,
        rdram: &[u8],
        vi: ViPresentation,
    ) -> Result<(), RenderError> {
        <Self as RenderBackend>::present(
            self,
            PresentRequest::physical_compatibility(
                vi,
                fn64_runtime::PhysicalRdramRead::from_storage(rdram),
            ),
        )
    }

    /// Platform-wide RT64 source/capture identity used by non-release behavior
    /// examples. On Windows this intentionally retains its historical
    /// D3D12-or-Vulkan label; fixed-cycle evidence must use
    /// [`Self::release_identity_for_api`] instead.
    ///
    /// The build script derives Git state from the selected source tree or
    /// records an explicit `FN64_RT64_SOURCE_ID` as declared provenance.
    #[cfg(feature = "rt64")]
    pub fn release_identity() -> Rt64BackendIdentity {
        Self::release_identity_with_post_vi_api(if cfg!(target_os = "macos") {
            "metal-bgra8-unorm"
        } else if cfg!(target_os = "windows") {
            "d3d12-or-vulkan-bgra8-rgba8-unorm"
        } else {
            "vulkan-bgra8-rgba8-unorm"
        })
    }

    /// Identity of the RT64 source and the concrete graphics API that owns
    /// the release image. Unlike [`Self::release_identity`], this cannot
    /// carry the legacy ambiguous Windows API label.
    #[cfg(feature = "rt64")]
    pub fn release_identity_for_api(api: ActiveRenderGraphicsApi) -> Rt64BackendIdentity {
        Self::release_identity_with_post_vi_api(post_vi_api_for_graphics_api(api))
    }

    /// Read the concrete graphics API owned by the successfully created RT64
    /// device. This evidence seam does no GPU work and requires no capture,
    /// present, or VI operation.
    pub fn live_device_graphics_api_for_evidence(
        &self,
    ) -> Result<ActiveRenderGraphicsApi, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_ref()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .live_device_graphics_api()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-live-device-api-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-live-device-api-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    #[cfg(feature = "rt64")]
    fn release_identity_with_post_vi_api(post_vi_api: &'static str) -> Rt64BackendIdentity {
        let source_provenance = match env!("FN64_RT64_SOURCE_PROVENANCE") {
            "git-clean" => Rt64SourceProvenance::GitClean,
            "git-dirty" => Rt64SourceProvenance::GitDirty,
            "declared" => Rt64SourceProvenance::Declared,
            value => panic!("unknown RT64 source provenance {value}"),
        };
        Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256: env!("FN64_RT64_ADAPTER_SOURCE_SHA256"),
            source_id: env!("FN64_RT64_SOURCE_ID"),
            source_provenance,
            source_overlay_id: env!("FN64_RT64_SOURCE_OVERLAY_ID"),
            post_vi_api,
        }
    }

    /// Enable exact post-VI swapchain render-target capture.
    ///
    /// The pinned RT64 generic render hook does not expose its framebuffer's
    /// attachment. This opt-in path validates the concrete Plume Metal,
    /// Vulkan, or D3D12 attachment and retains a fenced readback buffer.
    pub fn enable_present_capture(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_present_capture()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Wait for the sole/selected TMEM texture, optionally including its
    /// installed replacement. Completion is defined by RT64's live cache map;
    /// the C++ seam does not use a duration, sleep, or timing threshold.
    pub fn wait_texture_replacement_evidence(
        &mut self,
        texture_hash: Option<u64>,
        require_replacement: bool,
    ) -> Result<Rt64TextureReplacementEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .wait_texture_replacement_state(texture_hash, require_replacement)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-replacement-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (texture_hash, require_replacement);
            Err(RenderError::Backend {
                backend: "rt64-texture-replacement-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Pause or restore RT64's texture Stream workers for a deterministic
    /// behavior fixture. Pause succeeds only when the upload and stream queues
    /// are quiescent; resume recreates the exact pinned-cache worker count.
    /// This is an evidence scheduling gate, not renderer policy.
    pub fn set_texture_stream_workers_paused_for_evidence(
        &mut self,
        paused: bool,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .set_stream_workers_paused(paused)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-stream-evidence-control",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = paused;
            Err(RenderError::Backend {
                backend: "rt64-texture-stream-evidence-control",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Wait for a real RT64 Stream path to be resolved and queued while the
    /// evidence worker hold keeps its replacement absent from the texture map.
    pub fn wait_texture_stream_fallback_evidence(
        &mut self,
        texture_hash: u64,
    ) -> Result<Rt64TextureReplacementEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .wait_stream_fallback_state(texture_hash)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-texture-stream-fallback-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = texture_hash;
            Err(RenderError::Backend {
                backend: "rt64-texture-stream-fallback-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Read the most recent completed post-VI swapchain render target.
    pub fn presented_pixels(&mut self) -> Result<Rt64PresentedPixels, RenderError> {
        self.presented_pixels_into(&mut Vec::new())
    }

    /// Read the latest capture using storage recovered from a prior capture.
    /// The returned value owns that storage; errors leave `reuse` available to
    /// its caller.
    pub fn presented_pixels_into(
        &mut self,
        reuse: &mut Vec<u8>,
    ) -> Result<Rt64PresentedPixels, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let result = self
                .context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .presented_pixels_into(reuse);
            result.map_err(|reason| {
                if reason == "RT64 has no completed post-workload present capture" {
                    return RenderError::NotReady(
                        "RT64 has no completed post-workload present capture",
                    );
                }
                RenderError::Backend {
                    backend: "rt64-present-capture",
                    reason,
                }
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = reuse;
            Err(RenderError::Backend {
                backend: "rt64-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the exact source texture and framebuffer identity bound by RT64's
    /// most recently completed VI draw.
    pub fn present_selection(&mut self) -> Result<Rt64PresentSelection, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .present_selection()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-present-selection",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-present-selection",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm the next raw-DPC workload for a worker-excluded pre-submission
    /// snapshot. This evidence control is bounded to one completed workload.
    pub fn enable_deferred_workload_capture_for_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_deferred_workload_capture()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-deferred-workload-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-deferred-workload-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the captured pre-submission workload and its current paused-replay
    /// image after both RT64 queue workers become idle.
    pub fn deferred_workload_evidence(
        &mut self,
    ) -> Result<Rt64DeferredWorkloadEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .deferred_workload_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-deferred-workload-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-deferred-workload-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the exclusive GPU-tile-copy or CPU synchronization fallback route
    /// taken by the captured completed workload.
    pub fn framebuffer_copy_path_evidence(
        &mut self,
    ) -> Result<Rt64FramebufferCopyPathEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .framebuffer_copy_path_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-framebuffer-copy-path-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-framebuffer-copy-path-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read downstream texture-route vectors for the captured S2DEX workload.
    pub fn s2dex_fast_path_evidence(&mut self) -> Result<Rt64S2dexFastPathEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .s2dex_fast_path_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-s2dex-fast-path-evidence",
                    reason,
                })
        }
        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-s2dex-fast-path-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm pass-through typed evidence for exactly the next recognized-HLE
    /// task. This does not admit microcode or enable Extended GBI itself.
    pub fn enable_extended_gbi_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_extended_gbi_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-gbi-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-gbi-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the semantic Extended-GBI, aspect, vertex-Z, and generated-frame
    /// evidence bound to the explicitly armed completed workload.
    pub fn extended_gbi_evidence(&mut self) -> Result<Rt64ExtendedGbiEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .extended_gbi_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-gbi-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-gbi-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read every ordered post-VI image retained while the current Extended
    /// evidence interval was armed. Semantic evidence must be read first so
    /// the workload/present/fraction provenance has reached queue idle.
    pub fn extended_presented_pixels(
        &mut self,
    ) -> Result<Vec<Rt64ExtendedPresentedPixels>, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .extended_presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-extended-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-extended-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Arm exactly one bounded HFR presentation history.
    #[cfg(feature = "hfr-evidence")]
    pub fn enable_hfr_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_hfr_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM, hand-authored F3DEX2 display list for HFR evidence.
    ///
    /// Production [`RenderBackend::process_task`] recognition is deliberately
    /// unchanged; this method substitutes only the test fixture's microcode
    /// hash admission and then runs RT64's normal HLE/workload/render path.
    #[cfg(feature = "synthetic-f3dex2-evidence")]
    pub fn process_synthetic_hfr_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_hfr_f3dex2(
                    rdram,
                    display_list,
                    output_addr,
                    original_refresh_rate,
                )
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-hfr-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr, original_refresh_rate);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-hfr-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a hand-authored F3DEX2 display list without an Extended GBI
    /// refresh override and return the completed workload's inferred rate.
    ///
    /// This non-default evidence seam does not alter production microcode
    /// admission. Callers must drive ordinary VI events between submissions
    /// so RT64 can accumulate the stable-factor history used by FullSync.
    #[cfg(feature = "region-rate-evidence")]
    pub fn process_synthetic_region_rate_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<Rt64RegionRateEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_region_rate_f3dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-region-rate-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-region-rate-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM, hand-authored public S2DEX2 display list.
    ///
    /// This non-default evidence seam substitutes only the fixture's GBI
    /// dialect. Normal [`RenderBackend::process_task`] recognition continues
    /// to require an exact supported microcode identity.
    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub fn process_synthetic_s2dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_s2dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-s2dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-s2dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a non-ROM public legacy-S2DEX display list for negative evidence.
    ///
    /// This exists to prove bounded S2DEX2 adapter overlays do not accidentally
    /// broaden the shared upstream handler to the legacy wire family.
    #[cfg(feature = "synthetic-s2dex-evidence")]
    pub fn process_synthetic_legacy_s2dex_for_evidence(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_s2dex_wire(rdram, display_list, output_addr, true)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-legacy-s2dex",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-legacy-s2dex",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Process a hand-authored, non-ROM F3DEX2 display list through RT64's
    /// normal interpreter/workload/render path for Extended-GBI evidence.
    ///
    /// This non-default test seam substitutes the fixture's GBI dialect only.
    /// Production [`RenderBackend::process_task`] still requires RT64 to
    /// recognize the submitted microcode text/data pair by hash.
    #[cfg(feature = "extended-gbi-evidence")]
    pub fn process_synthetic_extended_f3dex2(
        &mut self,
        rdram: &mut [u8],
        display_list: u32,
        output_addr: u32,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .process_synthetic_extended_f3dex2(rdram, display_list, output_addr)
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-synthetic-extended-f3dex2",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (rdram, display_list, output_addr);
            Err(RenderError::Backend {
                backend: "rt64-synthetic-extended-f3dex2",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Finalize and read the causal HFR workload/presentation state.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_evidence(&mut self) -> Result<Rt64HfrEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .hfr_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read the ordered post-VI images after [`Self::hfr_evidence`] finalizes
    /// the associated workload and interpolation fractions.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_presented_pixels(&mut self) -> Result<Vec<Rt64HfrPresentedPixels>, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .hfr_presented_pixels()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-hfr-present-capture",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-hfr-present-capture",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Start a bounded observation window at RT64's actual present-call seam.
    #[cfg(feature = "hfr-evidence")]
    pub fn enable_hfr_pacing_evidence(&mut self) -> Result<(), RenderError> {
        self.context
            .as_mut()
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
            .enable_hfr_pacing_evidence()
            .map_err(|reason| RenderError::Backend {
                backend: "rt64-hfr-pacing-evidence",
                reason,
            })
    }

    /// Join both RT64 queues and finalize actual present-call timing evidence.
    ///
    /// This observes post-sleep call cadence, not physical display scanout.
    #[cfg(feature = "hfr-evidence")]
    pub fn hfr_pacing_evidence(&mut self) -> Result<Rt64HfrPacingEvidence, RenderError> {
        self.context
            .as_mut()
            .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
            .hfr_pacing_evidence()
            .map_err(|reason| RenderError::Backend {
                backend: "rt64-hfr-pacing-evidence",
                reason,
            })
    }

    /// Set the backend-independent debugger pause and render boundary used by
    /// pinned RT64's paused replay path.
    ///
    /// This is a deterministic host evidence seam, not a claim that RT64's
    /// ImGui Inspector frontend supports Metal.
    pub fn set_debugger_inspection_for_evidence(
        &mut self,
        paused: bool,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: bool,
    ) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .set_debugger_inspection_for_evidence(
                    paused,
                    framebuffer_index,
                    draw_call_index,
                    framebuffer_depth,
                )
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-debugger-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (
                paused,
                framebuffer_index,
                draw_call_index,
                framebuffer_depth,
            );
            Err(RenderError::Backend {
                backend: "rt64-debugger-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Wait for all eight pinned raster ubershader pipelines, force the
    /// backend's ubershader-only selection path, and begin exact Metal PSO
    /// construction-event evidence.
    pub fn enable_ubershader_evidence(&mut self) -> Result<(), RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .enable_ubershader_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-ubershader-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-ubershader-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Read construction counters and the exact precreated ubershader pipeline
    /// selected for every raster call in the most recently completed workload.
    pub fn ubershader_evidence(&mut self) -> Result<Rt64UbershaderEvidence, RenderError> {
        #[cfg(feature = "rt64")]
        {
            self.context
                .as_mut()
                .ok_or(RenderError::NotReady("Rt64Backend::create() not called"))?
                .ubershader_evidence()
                .map_err(|reason| RenderError::Backend {
                    backend: "rt64-ubershader-evidence",
                    reason,
                })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-ubershader-evidence",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature"
                    .to_string(),
            })
        }
    }

    /// Admit one exact task-entry F3DEX2 text image for RT64 HLE. Unknown
    /// images return `NeedsLle` without crossing the C ABI.
    pub fn with_f3dex2_ucode_sha256(mut self, digest: [u8; 32]) -> Self {
        self.f3dex2_ucodes.admit_sha256(digest);
        self
    }

    /// Admit one exact logical 4 KiB task-entry image, retaining only its
    /// SHA-256 identity. This mirrors the reference backend's fixture setup.
    pub fn with_f3dex2_ucode_text(mut self, text: &[u8]) -> Self {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "F3DEX2 text admission requires one complete 4 KiB IMEM image"
        );
        self.f3dex2_ucodes.admit_text(text);
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

    /// Compatibility helper for exact F3DEX2 runtime-recognition identity.
    pub fn with_f3dex2_ucode_pair_sha256(
        self,
        text_sha256: [u8; 32],
        data_bytes: u32,
        data_sha256: [u8; 32],
    ) -> Self {
        self.with_microcode_pair_sha256(UcodeId::F3dex2, text_sha256, data_bytes, data_sha256)
    }

    /// Compatibility helper for exact F3DEX2 runtime-recognition bytes.
    pub fn with_f3dex2_ucode_pair(self, text: &[u8], data: &[u8]) -> Self {
        self.with_microcode_pair(UcodeId::F3dex2, text, data)
    }

    /// Stage a complete settings image for the next backend creation.
    pub fn with_runtime_settings(mut self, settings: RenderRuntimeSettings) -> Self {
        self.configured_settings = settings;
        self
    }

    pub fn configured_settings(&self) -> &RenderRuntimeSettings {
        &self.configured_settings
    }

    pub fn active_settings(&self) -> Option<&RenderRuntimeSettings> {
        self.active_settings.as_ref()
    }

    pub fn with_enhancement_settings(mut self, settings: RenderEnhancementSettings) -> Self {
        self.configured_enhancement_settings = settings;
        self
    }

    pub fn with_emulator_settings(mut self, settings: RenderEmulatorSettings) -> Self {
        self.configured_emulator_settings = settings;
        self
    }

    pub fn with_runtime_policy(mut self, policy: RenderRuntimePolicy) -> Self {
        assert!(
            policy.replacement.packs.is_empty(),
            "with_runtime_policy cannot reconstruct replacement-pack host paths from byte identities; call load_replacement_packs before create"
        );
        self.configured_settings = policy.user;
        self.configured_enhancement_settings = policy.enhancement;
        self.configured_emulator_settings = policy.emulator;
        self.configured_replacement_packs.clear();
        self.configured_replacement_enabled = policy.replacement.enabled;
        self
    }

    pub fn configured_enhancement_settings(&self) -> &RenderEnhancementSettings {
        &self.configured_enhancement_settings
    }

    pub fn active_enhancement_settings(&self) -> Option<&RenderEnhancementSettings> {
        self.active_enhancement_settings.as_ref()
    }

    pub fn configured_emulator_settings(&self) -> &RenderEmulatorSettings {
        &self.configured_emulator_settings
    }

    pub fn active_emulator_settings(&self) -> Option<&RenderEmulatorSettings> {
        self.active_emulator_settings.as_ref()
    }

    pub fn configured_replacement_settings(&self) -> RenderReplacementSettings {
        RenderReplacementSettings {
            enabled: self.configured_replacement_enabled,
            packs: self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.identity.clone())
                .collect(),
        }
    }

    pub fn active_replacement_settings(&self) -> Option<&RenderReplacementSettings> {
        self.active_replacement_settings.as_ref()
    }

    pub fn configured_runtime_policy(&self) -> RenderRuntimePolicy {
        RenderRuntimePolicy {
            user: self.configured_settings.clone(),
            enhancement: self.configured_enhancement_settings.clone(),
            emulator: self.configured_emulator_settings.clone(),
            replacement: self.configured_replacement_settings(),
        }
    }

    pub fn active_runtime_policy(&self) -> Option<RenderRuntimePolicy> {
        Some(RenderRuntimePolicy {
            user: self.active_settings.as_ref()?.clone(),
            enhancement: self.active_enhancement_settings.as_ref()?.clone(),
            emulator: self.active_emulator_settings.as_ref()?.clone(),
            replacement: self.active_replacement_settings.as_ref()?.clone(),
        })
    }

    /// Inspect and stage ordered replacement packs, or transactionally load
    /// them into an existing RT64 context. Only a stable pre/load/post byte
    /// identity becomes active release policy.
    pub fn load_replacement_packs(
        &mut self,
        inputs: &[Rt64ReplacementPackInput],
        enabled: bool,
    ) -> Result<RenderPolicyApply, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let resolved =
                resolve_replacement_packs(inputs).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-inspect",
                    reason,
                })?;
            self.configured_replacement_packs = resolved.clone();
            self.configured_replacement_enabled = enabled;
            let configured_policy_sha = self.configured_runtime_policy().sha256();
            if self.context.is_none() {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: configured_policy_sha,
                });
            }
            let snapshot =
                create_replacement_snapshot(&resolved).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                })?;
            let native_replacements = snapshot
                .as_ref()
                .map_or([].as_slice(), |snapshot| snapshot.packs.as_slice());
            let ffi_inputs = replacement_ffi_inputs(native_replacements).map_err(|reason| {
                RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                }
            })?;
            if let Err(reason) = self
                .context
                .as_mut()
                .expect("context presence was checked")
                .load_replacement_packs(&ffi_inputs, enabled)
            {
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason,
                });
            }
            let snapshot_inputs: Vec<_> = native_replacements
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let after = match resolve_replacement_packs(&snapshot_inputs) {
                Ok(after) => after,
                Err(reason) => {
                    self.invalidate_native_state();
                    return Err(RenderError::Backend {
                        backend: "rt64-replacement-load",
                        reason,
                    });
                }
            };
            if !replacement_identities_match(&resolved, &after) {
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-load",
                    reason: "replacement snapshot bytes changed while RT64 activated it".into(),
                });
            }
            self.active_replacement_snapshot = snapshot;
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled,
                packs: resolved.into_iter().map(|pack| pack.identity).collect(),
            });
            let policy_sha256 = self
                .active_runtime_policy()
                .ok_or(RenderError::NotReady(
                    "RT64 replacement load has no complete active runtime policy",
                ))?
                .sha256();
            Ok(RenderPolicyApply::LiveApplied { policy_sha256 })
        }

        #[cfg(not(feature = "rt64"))]
        {
            let _ = (inputs, enabled);
            Err(RenderError::Backend {
                backend: "rt64-replacement-inspect",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    /// Re-inspect and reload the currently configured ordered pack paths.
    pub fn reload_replacement_packs(&mut self) -> Result<RenderPolicyApply, RenderError> {
        #[cfg(feature = "rt64")]
        {
            let inputs: Vec<_> = self
                .configured_replacement_packs
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let enabled = self.configured_replacement_enabled;
            let resolved =
                resolve_replacement_packs(&inputs).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                })?;
            self.configured_replacement_packs = resolved.clone();
            if self.context.is_none() {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            }
            let snapshot =
                create_replacement_snapshot(&resolved).map_err(|reason| RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                })?;
            let native_replacements = snapshot
                .as_ref()
                .map_or([].as_slice(), |snapshot| snapshot.packs.as_slice());
            let ffi_inputs = replacement_ffi_inputs(native_replacements).map_err(|reason| {
                RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                }
            })?;
            if let Err(reason) = self
                .context
                .as_mut()
                .expect("context presence was checked")
                .reload_replacement_packs(&ffi_inputs, enabled)
            {
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason,
                });
            }
            let snapshot_inputs: Vec<_> = native_replacements
                .iter()
                .map(|pack| pack.input.clone())
                .collect();
            let after = match resolve_replacement_packs(&snapshot_inputs) {
                Ok(after) => after,
                Err(reason) => {
                    self.invalidate_native_state();
                    return Err(RenderError::Backend {
                        backend: "rt64-replacement-reload",
                        reason,
                    });
                }
            };
            if !replacement_identities_match(&resolved, &after) {
                self.invalidate_native_state();
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-reload",
                    reason: "replacement snapshot bytes changed while RT64 reloaded it".into(),
                });
            }
            self.active_replacement_snapshot = snapshot;
            self.active_replacement_settings = Some(RenderReplacementSettings {
                enabled,
                packs: resolved.into_iter().map(|pack| pack.identity).collect(),
            });
            Ok(RenderPolicyApply::LiveApplied {
                policy_sha256: self
                    .active_runtime_policy()
                    .ok_or(RenderError::NotReady(
                        "RT64 replacement reload has no complete active runtime policy",
                    ))?
                    .sha256(),
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Err(RenderError::Backend {
                backend: "rt64-replacement-reload",
                reason: "fn64-render-rt64 was built without the opt-in `rt64` Cargo feature".into(),
            })
        }
    }

    pub fn set_replacements_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<RenderPolicyApply, RenderError> {
        self.configured_replacement_enabled = enabled;
        #[cfg(feature = "rt64")]
        {
            let Some(context) = self.context.as_mut() else {
                return Ok(RenderPolicyApply::StagedForCreate {
                    policy_sha256: self.configured_runtime_policy().sha256(),
                });
            };
            if let Err(reason) = context.set_replacement_enabled(enabled) {
                self.active_replacement_settings = None;
                return Err(RenderError::Backend {
                    backend: "rt64-replacement-enable",
                    reason,
                });
            }
            let active = self
                .active_replacement_settings
                .as_mut()
                .ok_or(RenderError::NotReady(
                    "RT64 replacement enable has no active pack identity",
                ))?;
            active.enabled = enabled;
            Ok(RenderPolicyApply::LiveApplied {
                policy_sha256: self
                    .active_runtime_policy()
                    .ok_or(RenderError::NotReady(
                        "RT64 replacement enable has no complete active runtime policy",
                    ))?
                    .sha256(),
            })
        }

        #[cfg(not(feature = "rt64"))]
        {
            Ok(RenderPolicyApply::StagedForCreate {
                policy_sha256: self.configured_runtime_policy().sha256(),
            })
        }
    }
}
