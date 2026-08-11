use super::*;


    #[cfg(not(feature = "rt64"))]
    #[test]
    fn no_native_build_rejects_preflighted_batch_without_mutation() {
        let submission = fn64_render::OwnedRawDpcSubmission::from_rdram_words(
            0x100,
            0x108,
            vec![0xe900_0000, 0],
        )
        .unwrap();
        let mut rdram = vec![0x5a; 0x400];
        let before = rdram.clone();
        let batch = fn64_render::RawDpcBatch::new(vec![submission])
            .unwrap()
            .preflight(rdram.len())
            .unwrap();
        let mut backend = Rt64Backend::new();

        let error = backend
            .process_raw_dpc_batch(&mut rdram, batch, 0)
            .unwrap_err();

        assert!(matches!(error, RenderError::Backend { .. }));
        assert!(error
            .to_string()
            .contains("native separate-command-buffer seam"));
        assert_eq!(
            backend.raw_dpc_batch_capability(),
            fn64_render::RawDpcBatchCapability::Unsupported
        );
        assert_eq!(rdram, before);
        assert_eq!(
            backend.last_dp_full_sync(),
            fn64_render::DpFullSyncStatus::Unidentified
        );
    }

    #[test]
    fn rt64_process_task_has_no_reference_decoder_paths() {
        let source = include_str!("lib.rs");
        let rt64_impl = source
            .find("impl RenderBackend for Rt64Backend")
            .expect("Rt64Backend RenderBackend implementation exists");
        let process_start = rt64_impl
            + source[rt64_impl..]
                .find("    fn process_task(")
                .expect("Rt64Backend process_task exists");
        let process_end = process_start
            + source[process_start..]
                .find("    fn process_rdp_commands(")
                .expect("process_rdp_commands follows process_task");
        let process_task = &source[process_start..process_end];

        assert_eq!(
            process_task
                .matches("fn64_render::inspect_geometry_task(")
                .count(),
            1,
            "RT64 production task submission must have one shared admission walk"
        );
        assert!(process_task.contains("f3dex_force_branch"));
        assert!(process_task.contains("GeometryTaskInspectionPolicy { force_branch }"));
        assert!(process_task.contains("NativeContextLease::take(&mut self.context)"));
        assert!(process_task.contains("NativeTaskMemoryRollback::new("));
        assert!(process_task.contains("&mut self.native_rdram_preimage"));
        assert!(process_task.contains("transaction.commit()"));
        assert!(
            !process_task.contains("self.context.as_mut()"),
            "native task execution must take context ownership before FFI"
        );
        for forbidden in [
            "gbi::inspect_",
            "gbi::execute_",
            "gbi::decode_",
            "gbi::trace_",
            "gbi::RenderOp",
        ] {
            assert!(
                !process_task.contains(forbidden),
                "RT64 production task submission still references {forbidden}"
            );
        }
    }

    #[test]
    fn rt64_raw_rdp_submission_owns_context_and_invalidates_on_failure() {
        // No RDRAM rollback here anymore (2026-08-10): the only path that
        // would read the pre-image is the `Err` arm, which calls
        // `invalidate_native_state()` and tears down `self.context` before
        // returning -- no caller ever resumes against the RDRAM this
        // submission touched after a failure, so restoring it was pure cost.
        // Measured on the render-benchmark route (rt64 lane, 4,032-call
        // sample): rollback's `copy_from_slice`/`extend_from_slice` over the
        // full RDRAM image cost ~0.125ms/call against ~1.088ms/call for the
        // RT64 FFI itself -- real, and now gone. What still must hold: the
        // context is taken (owned) before the FFI call, and a failure still
        // invalidates the native session rather than leaving it half-applied.
        let source = include_str!("lib.rs");
        let rt64_impl = source
            .find("impl RenderBackend for Rt64Backend")
            .expect("Rt64Backend RenderBackend implementation exists");
        let process_start = rt64_impl
            + source[rt64_impl..]
                .find("    fn process_rdp_commands(")
                .expect("Rt64Backend process_rdp_commands exists");
        let process_end = process_start
            + source[process_start..]
                .find("    fn last_dp_full_sync(")
                .expect("last_dp_full_sync follows process_rdp_commands");
        let process_rdp = &source[process_start..process_end];

        assert!(process_rdp.contains("NativeContextLease::take(&mut self.context)"));
        assert!(
            process_rdp.contains("self.invalidate_native_state()"),
            "a failed raw RDP submission must invalidate the native session, \
             not leave it in a half-applied state"
        );
        assert!(
            !process_rdp.contains("NativeRdramRollback::new("),
            "raw RDP submission must not construct an RDRAM rollback -- its \
             only consumer (the failure path) tears down the whole native \
             context, so restoring bytes for it is dead weight"
        );
        assert!(
            !process_rdp.contains("self.context.as_mut()"),
            "raw RDP execution must take context ownership before FFI"
        );
    }

    #[test]
    fn native_full_sync_count_comparison_is_exact_not_boolean() {
        for (count, expected) in [
            (0, fn64_render::DpFullSyncStatus::NotReached),
            (1, fn64_render::DpFullSyncStatus::Reached),
            (3, fn64_render::DpFullSyncStatus::Reached),
        ] {
            assert_eq!(validate_native_full_sync_count(count, count), Ok(expected));
        }
        for (inspected, native) in [(1, 2), (2, 1)] {
            let error = validate_native_full_sync_count(inspected, native).unwrap_err();
            assert!(error.contains(&format!("executed {native} FullSync")));
            assert!(error.contains(&format!("executed {inspected}")));
        }
    }

    #[test]
    fn native_invalidation_clears_active_identity_but_keeps_recreate_configuration() {
        let mut backend = Rt64Backend::new();
        let configured_policy_sha256 = backend.configured_runtime_policy().sha256();
        backend.active_tv_type = Some(fn64_runtime::TvType::Pal);
        backend.last_present = Some(CompletedRt64Present {
            guest_cycle: 91,
            authority: Rt64PresentAuthority::LiveRegisters,
        });
        backend.active_settings = Some(RenderRuntimeSettings::default());
        backend.active_enhancement_settings = Some(RenderEnhancementSettings::default());
        backend.active_emulator_settings = Some(RenderEmulatorSettings::default());
        backend.active_replacement_settings = Some(RenderReplacementSettings::default());
        backend.last_dp_full_sync = fn64_render::DpFullSyncStatus::Reached;

        backend.clear_active_native_identity();

        assert_eq!(backend.active_tv_type, None);
        assert_eq!(backend.last_present, None);
        assert_eq!(backend.active_settings, None);
        assert_eq!(backend.active_enhancement_settings, None);
        assert_eq!(backend.active_emulator_settings, None);
        assert_eq!(backend.active_replacement_settings, None);
        assert_eq!(
            backend.last_dp_full_sync,
            fn64_render::DpFullSyncStatus::Unidentified
        );
        assert_eq!(
            backend.configured_runtime_policy().sha256(),
            configured_policy_sha256
        );
    }

    #[test]
    fn rt64_release_authority_rejects_backend_only_compatibility() {
        let compatibility = CompletedRt64Present {
            guest_cycle: 17,
            authority: Rt64PresentAuthority::BackendOnlyCompatibility,
        };
        assert!(matches!(
            compatibility.release_guest_cycle(),
            Err(RenderError::NotReady(
                "RT64 release capture requires a completed live-register VI present"
            ))
        ));
        let live = CompletedRt64Present {
            guest_cycle: 19,
            authority: Rt64PresentAuthority::LiveRegisters,
        };
        assert_eq!(live.release_guest_cycle().unwrap(), 19);
    }

    #[cfg(feature = "rt64")]
    struct SyntheticPack(PathBuf);

    #[cfg(feature = "rt64")]
    impl SyntheticPack {
        fn new(name: &str, auto_path: &str, operation: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "fn64-rt64-pack-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                name
            ));
            std::fs::create_dir(&path).expect("create synthetic replacement pack");
            let database = format!(
                "{{\"configuration\":{{\"configurationVersion\":3,\"autoPath\":\"{auto_path}\",\"defaultOperation\":\"{operation}\",\"defaultShift\":\"half\",\"hashVersion\":5}},\"textures\":[],\"operationFilters\":[],\"shiftFilters\":[],\"extraFiles\":[]}}"
            );
            std::fs::write(path.join("rt64.json"), database)
                .expect("write synthetic replacement database");
            Self(path)
        }

        fn input(&self) -> Rt64ReplacementPackInput {
            Rt64ReplacementPackInput::new(&self.0)
        }
    }

    #[cfg(feature = "rt64")]
    impl Drop for SyntheticPack {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove synthetic replacement pack");
        }
    }

    #[test]
    fn explicit_graphics_api_request_must_match_the_observed_capture_backend() {
        for (requested, observed) in [
            (RenderGraphicsApi::D3d12, ActiveRenderGraphicsApi::D3d12),
            (RenderGraphicsApi::Vulkan, ActiveRenderGraphicsApi::Vulkan),
            (RenderGraphicsApi::Metal, ActiveRenderGraphicsApi::Metal),
        ] {
            assert!(graphics_api_matches_request(requested, observed));
            for other in [
                ActiveRenderGraphicsApi::D3d12,
                ActiveRenderGraphicsApi::Vulkan,
                ActiveRenderGraphicsApi::Metal,
            ] {
                assert_eq!(
                    graphics_api_matches_request(requested, other),
                    other == observed
                );
            }
        }
    }

    #[test]
    fn release_post_vi_api_identity_is_concrete_and_api_specific() {
        assert_eq!(
            post_vi_api_for_graphics_api(ActiveRenderGraphicsApi::D3d12),
            "d3d12-bgra8-rgba8-unorm"
        );
        assert_eq!(
            post_vi_api_for_graphics_api(ActiveRenderGraphicsApi::Vulkan),
            "vulkan-bgra8-rgba8-unorm"
        );
        assert_eq!(
            post_vi_api_for_graphics_api(ActiveRenderGraphicsApi::Metal),
            "metal-bgra8-unorm"
        );
        for api in [
            ActiveRenderGraphicsApi::D3d12,
            ActiveRenderGraphicsApi::Vulkan,
            ActiveRenderGraphicsApi::Metal,
        ] {
            assert!(!post_vi_api_for_graphics_api(api).contains("-or-"));
        }
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn release_backend_identity_binds_the_concrete_api() {
        for (api, expected) in [
            (ActiveRenderGraphicsApi::D3d12, "d3d12-bgra8-rgba8-unorm"),
            (ActiveRenderGraphicsApi::Vulkan, "vulkan-bgra8-rgba8-unorm"),
            (ActiveRenderGraphicsApi::Metal, "metal-bgra8-unorm"),
        ] {
            let identity = Rt64Backend::release_identity_for_api(api);
            assert_eq!(identity.post_vi_api, expected);
            assert!(identity.canonical_id().contains(expected));
            assert!(!identity.canonical_id().contains("d3d12-or-vulkan"));
        }
    }

    #[test]
    fn automatic_graphics_api_evidence_accepts_only_the_observed_capture_backend() {
        for observed in [
            ActiveRenderGraphicsApi::D3d12,
            ActiveRenderGraphicsApi::Vulkan,
            ActiveRenderGraphicsApi::Metal,
        ] {
            assert!(graphics_api_matches_request(
                RenderGraphicsApi::Automatic,
                observed
            ));
        }
    }

    #[test]
    fn rt64_release_environment_requires_a_completed_observed_capture_backend() {
        let backend = Rt64Backend::new();
        assert_eq!(
            backend.release_environment(),
            fn64_render::RenderBackendEvidence::Unidentified
        );
        assert_eq!(backend.release_environment().tv_type(), None);
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn replacement_pack_inspection_is_ordered_typed_and_staged_without_active_evidence() {
        let first = SyntheticPack::new("first", "rt64", "preload");
        let second = SyntheticPack::new("second", "rice", "stall");
        std::fs::write(first.0.join("extra.bin"), b"first-content")
            .expect("write synthetic pack content");

        let mut backend = Rt64Backend::new();
        let inputs = [first.input(), second.input()];
        let applied = backend.load_replacement_packs(&inputs, false).unwrap();
        let replacement = backend.configured_replacement_settings();
        assert!(!replacement.enabled);
        assert_eq!(replacement.packs.len(), 2);
        assert_eq!(
            replacement.packs[0].auto_path,
            fn64_render::RenderReplacementAutoPath::Rt64
        );
        assert_eq!(
            replacement.packs[0].default_operation,
            fn64_render::RenderReplacementOperation::Preload
        );
        assert_eq!(
            replacement.packs[1].auto_path,
            fn64_render::RenderReplacementAutoPath::Rice
        );
        assert_eq!(
            replacement.packs[1].default_operation,
            fn64_render::RenderReplacementOperation::Stall
        );
        assert_ne!(
            replacement.packs[0].content_sha256,
            replacement.packs[1].content_sha256
        );
        assert_ne!(
            replacement.packs[0].database_sha256,
            replacement.packs[1].database_sha256
        );
        assert_eq!(backend.active_replacement_settings(), None);
        assert_eq!(
            applied,
            RenderPolicyApply::StagedForCreate {
                policy_sha256: backend.configured_runtime_policy().sha256()
            }
        );

        let reversed = resolve_replacement_packs(&[second.input(), first.input()]).unwrap();
        let reversed_policy = RenderReplacementSettings {
            enabled: false,
            packs: reversed.into_iter().map(|pack| pack.identity).collect(),
        };
        assert_ne!(replacement.sha256(), reversed_policy.sha256());
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn replacement_pack_inspection_rejects_ambiguous_or_silently_ignored_inputs() {
        let pack = SyntheticPack::new("duplicate", "rt64", "stream");
        let duplicate = resolve_replacement_packs(&[pack.input(), pack.input()]).unwrap_err();
        assert!(duplicate.contains("duplicated"));

        std::fs::write(
            pack.0.join("rt64.json"),
            b"{\"configuration\":{\"hashVersion\":999}}",
        )
        .expect("write unsupported synthetic database");
        let unsupported = resolve_replacement_packs(&[pack.input()]).unwrap_err();
        assert!(unsupported.contains("newer than pinned RT64"));

        std::fs::write(
            pack.0.join("rt64.json"),
            b"{\"configuration\":{\"autoPath\":\"guess\"}}",
        )
        .expect("write ambiguous synthetic database");
        let ambiguous = resolve_replacement_packs(&[pack.input()]).unwrap_err();
        assert!(ambiguous.contains("unknown autoPath"));
    }

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_backend_without_feature_is_a_named_error_not_a_silent_success() {
        let mut backend = Rt64Backend::new();
        assert_eq!(
            backend.task_chunking(),
            fn64_render::RenderTaskChunking::Atomic
        );
        let err = backend.create(&RenderConfig::ntsc(320, 240)).unwrap_err();
        match err {
            RenderError::Backend { backend, .. } => assert_eq!(backend, "rt64"),
            other => panic!("expected Backend stub error, got {other:?}"),
        }
        assert!(!backend.created);
        assert_eq!(backend.release_environment().tv_type(), None);
        assert!(backend.supported_ucodes().is_empty());
    }

    #[test]
    fn rt64_backend_identifies_only_exact_admitted_imem_images() {
        let admitted = [0x81; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let unadmitted = [0x82; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = Rt64Backend::new().with_f3dex2_ucode_text(&admitted);

        assert_eq!(backend.identify_microcode(&admitted), Some(UcodeId::F3dex2));
        assert_eq!(backend.identify_microcode(&unadmitted), None);
    }

    #[test]
    fn rt64_pair_recognition_requires_exact_text_data_length_and_digest() {
        let text = [0x81; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let other_text = [0x82; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x31, 0x41, 0x59, 0x26, 0x53];
        let identity = MicrocodeDataImageIdentity {
            bytes: data.len() as u32,
            sha256: sha2::Sha256::digest(data).into(),
        };
        let text_only = Rt64Backend::new().with_f3dex2_ucode_text(&text);
        assert_eq!(text_only.identify_microcode_pair(&text, identity), None);

        let backend = text_only.with_f3dex2_ucode_pair(&text, &data);
        assert_eq!(
            backend.identify_microcode_pair(&text, identity),
            Some(UcodeId::F3dex2)
        );
        assert_eq!(backend.identify_microcode_pair(&other_text, identity), None);
        assert_eq!(
            backend.identify_microcode_pair(
                &text,
                MicrocodeDataImageIdentity {
                    bytes: identity.bytes + 1,
                    ..identity
                }
            ),
            None
        );
        assert_eq!(
            backend.identify_microcode_pair(
                &text,
                MicrocodeDataImageIdentity {
                    sha256: [0xff; 32],
                    ..identity
                }
            ),
            None
        );
    }

    #[test]
    fn rt64_f3dzex2_pair_recognition_does_not_admit_hle() {
        let text = [0x7a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let other_text = [0x7b; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x5a, 0x45, 0x58, 0x32];
        let identity = MicrocodeDataImageIdentity {
            bytes: data.len() as u32,
            sha256: sha2::Sha256::digest(data).into(),
        };
        let backend = Rt64Backend::new().with_microcode_pair(UcodeId::F3dzex2, &text, &data);

        assert_eq!(
            backend.identify_microcode_pair(&text, identity),
            Some(UcodeId::F3dzex2)
        );
        assert_eq!(backend.identify_microcode_pair(&other_text, identity), None);
        assert_eq!(
            backend.identify_microcode_pair(
                &text,
                MicrocodeDataImageIdentity {
                    sha256: [0xff; 32],
                    ..identity
                }
            ),
            None
        );
        assert_eq!(backend.identify_microcode(&text), None);
        assert!(backend.supported_ucodes().is_empty());
    }

    #[test]
    #[cfg(feature = "rt64")]
    fn rt64_shared_task_entry_plan_binds_native_rdram_to_admitted_live_imem() {
        const TEXT: u32 = 0x1000;
        const DATA: u32 = 0x3000;
        const DL: u32 = 0x4800;
        let text = [0x73; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = [0x29; 8];
        let mut rdram = vec![0u8; 0x5000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT), &text);
            view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA), &data);
            view.write_u32(fn64_runtime::RdramAddr::from_offset(DL), 0xdf00_0000);
            view.write_u32(fn64_runtime::RdramAddr::from_offset(DL + 4), 0);
        }
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &text,
            )
            .unwrap();
        let task = OsTask {
            ucode: TEXT,
            ucode_data: DATA,
            ucode_data_size: data.len() as u32,
            data_ptr: DL,
            ..OsTask::default()
        };
        let mut catalog = F3dex2UcodeCatalog::default();
        catalog.admit_text(&text);
        let inspection = fn64_render::inspect_geometry_task(
            &rdram,
            &rsp_memory,
            &task,
            &catalog,
            fn64_render::GeometryTaskInspectionPolicy::default(),
            Some(fn64_render::TaskAdmissionRawWindowSize {
                text: RT64_GBI_TEXT_RECOGNITION_BYTES,
                data: RT64_GBI_DATA_RECOGNITION_BYTES,
            }),
        )
        .unwrap();
        let plan = Rt64TaskAdmission {
            plan: inspection.admission_plan,
            raw_windows: inspection.raw_windows,
        };
        assert_eq!(plan.plan.len(), 1);
        assert_eq!(
            plan.plan.entry().source,
            fn64_render::TaskAdmissionSource::TaskEntry
        );
        assert_eq!(
            plan.plan.entry().text_sha256,
            fn64_render::UcodeDigest::from_text(&text)
        );
        let data_sha256: [u8; 32] = sha2::Sha256::digest(data).into();
        assert_eq!(plan.plan.entry().data.sha256, data_sha256);
        assert_eq!(plan.raw_windows.len(), 1);
        assert_eq!(
            plan.raw_windows[0].text,
            rdram[TEXT as usize..TEXT as usize + RT64_GBI_TEXT_RECOGNITION_BYTES]
        );

        rdram[TEXT as usize ^ 3] ^= 0xff;
        let mismatch = fn64_render::inspect_geometry_task(
            &rdram,
            &rsp_memory,
            &task,
            &catalog,
            fn64_render::GeometryTaskInspectionPolicy::default(),
            Some(fn64_render::TaskAdmissionRawWindowSize {
                text: RT64_GBI_TEXT_RECOGNITION_BYTES,
                data: RT64_GBI_DATA_RECOGNITION_BYTES,
            }),
        )
        .unwrap_err();
        assert!(matches!(
            mismatch,
            RenderError::RequiresLle { ucode_sha256 }
                if ucode_sha256 == fn64_render::UcodeDigest::from_text(&text).as_bytes()
        ));
    }

    #[test]
    #[cfg(not(feature = "rt64"))]
    fn rt64_settings_stage_before_create_without_claiming_an_active_image() {
        let mut backend = Rt64Backend::new();
        let settings = RenderRuntimeSettings::upstream_default();
        assert_eq!(
            backend.apply_runtime_settings(&settings).unwrap(),
            RenderSettingsApply::StagedForCreate {
                settings_sha256: settings.sha256()
            }
        );
        assert_eq!(backend.configured_settings(), &settings);
        assert_eq!(backend.active_settings(), None);

        let enhancement = RenderEnhancementSettings::upstream_default();
        let expected_policy = RenderRuntimePolicy {
            user: settings,
            enhancement: enhancement.clone(),
            emulator: RenderEmulatorSettings::default(),
            replacement: fn64_render::RenderReplacementSettings::default(),
        };
        assert_eq!(
            backend.apply_enhancement_settings(&enhancement).unwrap(),
            RenderPolicyApply::StagedForCreate {
                policy_sha256: expected_policy.sha256()
            }
        );
        let emulator = RenderEmulatorSettings {
            post_blend_noise: false,
            ..RenderEmulatorSettings::default()
        };
        let expected_policy = RenderRuntimePolicy {
            emulator: emulator.clone(),
            ..expected_policy
        };
        assert_eq!(
            backend.apply_emulator_settings(&emulator).unwrap(),
            RenderPolicyApply::StagedForCreate {
                policy_sha256: expected_policy.sha256()
            }
        );
        assert_eq!(backend.configured_runtime_policy(), expected_policy);
        assert_eq!(backend.active_runtime_policy(), None);
    }

    #[test]
    fn backend_identity_binds_fn64_adapter_source_sha256() {
        let baseline = Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256:
                "1111111111111111111111111111111111111111111111111111111111111111",
            source_id: "git:2222222222222222222222222222222222222222",
            source_provenance: Rt64SourceProvenance::GitClean,
            source_overlay_id: "fn64:test-overlay:v1",
            post_vi_api: "metal-bgra8-unorm",
        };
        let changed = Rt64BackendIdentity {
            adapter_source_sha256:
                "3333333333333333333333333333333333333333333333333333333333333333",
            ..baseline.clone()
        };
        assert_ne!(baseline.canonical_id(), changed.canonical_id());
        assert!(baseline
            .canonical_id()
            .contains("adapter_sha256=1111111111111111"));
    }
