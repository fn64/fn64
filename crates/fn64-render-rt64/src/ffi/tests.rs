use super::*;

    use super::*;

    #[test]
    fn native_task_result_derives_full_sync_count_and_ucode_transition() {
        let plan_sha256 = [0x42; 32];
        let outcome = task_result_from_raw(
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                workload_id_before: 7,
                workload_id_after: 10,
                initial_ucode_text_address: 0x1000,
                initial_ucode_data_address: 0x2000,
                final_ucode_text_address: 0x3000,
                final_ucode_data_address: 0x4000,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 3,
                observed_generation_count: 3,
                rejected_generation: u32::MAX,
                plan_sha256,
            },
            3,
            plan_sha256,
        )
        .unwrap();
        let NativeTaskOutcome::Complete(result) = outcome else {
            panic!("complete native result decoded as NeedsLle");
        };
        assert_eq!(result.dp_full_sync, DpFullSyncStatus::Reached);
        assert_eq!(result.workload_id_before, 7);
        assert_eq!(result.workload_id_after, 10);
        assert_eq!(result.full_sync_count, 3);
        assert_eq!(result.initial_ucode_addresses, (0x1000, 0x2000));
        assert_eq!(result.final_ucode_addresses, (0x3000, 0x4000));
        assert_eq!(result.planned_generation_count, 3);
        assert_eq!(result.observed_generation_count, 3);
        assert_eq!(result.plan_sha256, plan_sha256);

        let no_sync = task_result_from_raw(
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                workload_id_before: 11,
                workload_id_after: 11,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            1,
            plan_sha256,
        )
        .unwrap();
        let NativeTaskOutcome::Complete(no_sync) = no_sync else {
            panic!("complete no-sync result decoded as NeedsLle");
        };
        assert_eq!(no_sync.dp_full_sync, DpFullSyncStatus::NotReached);
        assert_eq!(no_sync.workload_id_before, 11);
        assert_eq!(no_sync.workload_id_after, 11);
        assert_eq!(no_sync.full_sync_count, 0);
    }

    #[test]
    fn native_task_result_preserves_precommit_needs_lle_generation() {
        let plan_sha256 = [0x31; 32];
        let outcome = task_result_from_raw(
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                disposition: UCODE_DISPOSITION_NEEDS_LLE,
                planned_generation_count: 3,
                observed_generation_count: 0,
                rejected_generation: 1,
                plan_sha256,
                ..RawTaskResult::default()
            },
            3,
            plan_sha256,
        )
        .unwrap();
        assert_eq!(
            outcome,
            NativeTaskOutcome::NeedsLle {
                rejected_generation: 1,
                plan_sha256
            }
        );
    }

    #[test]
    fn native_task_result_rejects_untyped_or_inconsistent_success() {
        let plan_sha256 = [0x42; 32];
        for raw in [
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA + 1,
                entry_gbi_available: 1,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 0,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                workload_id_before: 2,
                workload_id_after: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 2,
                observed_generation_count: 2,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256: [0x24; 32],
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_COMPLETE,
                planned_generation_count: 1,
                observed_generation_count: 2,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: UCODE_DISPOSITION_NEEDS_LLE,
                planned_generation_count: 1,
                rejected_generation: 0,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                disposition: UCODE_DISPOSITION_NEEDS_LLE,
                planned_generation_count: 1,
                rejected_generation: 1,
                plan_sha256,
                ..RawTaskResult::default()
            },
            RawTaskResult {
                schema: TASK_RESULT_SCHEMA,
                entry_gbi_available: 1,
                disposition: 99,
                planned_generation_count: 1,
                observed_generation_count: 1,
                rejected_generation: u32::MAX,
                plan_sha256,
                ..RawTaskResult::default()
            },
        ] {
            assert!(task_result_from_raw(raw, 1, plan_sha256).is_err());
        }
    }

    #[test]
    fn native_ucode_plan_binds_ordered_logical_and_raw_recognition_images() {
        let generation = |source, text_address, digest| fn64_render::TaskAdmissionGeneration {
            source,
            text_address,
            data_address: 0x4000,
            text_sha256: fn64_render::UcodeDigest::from_sha256([digest; 32]),
            data: fn64_render::MicrocodeDataImageIdentity {
                bytes: 8,
                sha256: [digest.wrapping_add(1); 32],
            },
            ucode: TaskAdmissionUcode::F3dex2,
        };
        let entry = generation(fn64_render::TaskAdmissionSource::TaskEntry, 0x1000, 0x11);
        let self_load = generation(fn64_render::TaskAdmissionSource::SelfLoad, 0x2000, 0x22);
        let raw_window = |byte| fn64_render::TaskAdmissionRawWindow {
            text: vec![byte; crate::RT64_GBI_TEXT_RECOGNITION_BYTES],
            data: vec![byte.wrapping_add(1); crate::RT64_GBI_DATA_RECOGNITION_BYTES],
        };
        let admission = crate::Rt64TaskAdmission {
            plan: fn64_render::TaskAdmissionPlan::new(entry, [self_load]),
            raw_windows: vec![raw_window(0x31), raw_window(0x42)].into_boxed_slice(),
        };
        let prepared = PreparedUcodePlan::new(&admission).unwrap();
        assert_eq!(prepared.generations.len(), 2);
        assert_eq!(prepared.generations[0].source, UCODE_SOURCE_TASK_ENTRY);
        assert_eq!(prepared.generations[1].source, UCODE_SOURCE_SELF_LOAD);
        assert_eq!(prepared.generations[0].raw_text_offset, 0);
        assert_eq!(
            prepared.generations[0].raw_data_offset as usize,
            crate::RT64_GBI_TEXT_RECOGNITION_BYTES
        );
        assert_eq!(prepared.raw().plan_sha256, prepared.plan_sha256);

        let mut opaque_entry = entry;
        opaque_entry.ucode = TaskAdmissionUcode::Other(0x5645_4e44);
        let opaque = PreparedUcodePlan::new(&crate::Rt64TaskAdmission {
            plan: fn64_render::TaskAdmissionPlan::new(opaque_entry, []),
            raw_windows: vec![raw_window(0x53)].into_boxed_slice(),
        })
        .unwrap();
        assert_eq!(opaque.generations[0].expected_family, 0);
        assert_eq!(opaque.generations[0].expected_detail, 0x5645_4e44);

        let mut changed = admission;
        changed.raw_windows[1].text[0] ^= 0xff;
        assert_ne!(
            prepared.plan_sha256,
            PreparedUcodePlan::new(&changed).unwrap().plan_sha256
        );
    }

    #[test]
    fn native_ucode_identity_keeps_every_f3dzex2_variant_typed() {
        assert_eq!(raw_ucode_identity(TaskAdmissionUcode::F3dex2), (5, 0));
        for (variant, detail) in [
            (fn64_render::F3dzex2Variant::NoNFifo206H, 1),
            (fn64_render::F3dzex2Variant::NoNFifo208I, 2),
            (fn64_render::F3dzex2Variant::NoNFifo208J, 3),
        ] {
            let typed = TaskAdmissionUcode::F3dzex2(variant);
            assert_eq!(raw_ucode_identity(typed), (9, detail));
            assert!(validate_f3dzex2_profile(typed, Some(variant)).is_ok());
        }
        assert!(validate_f3dzex2_profile(
            TaskAdmissionUcode::F3dzex2(fn64_render::F3dzex2Variant::NoNFifo206H),
            Some(fn64_render::F3dzex2Variant::NoNFifo208I),
        )
        .is_err());
        assert!(validate_f3dzex2_profile(
            TaskAdmissionUcode::F3dex2,
            Some(fn64_render::F3dzex2Variant::NoNFifo208J)
        )
        .is_err());
        assert!(validate_f3dzex2_profile(
            TaskAdmissionUcode::F3dzex2(fn64_render::F3dzex2Variant::NoNFifo208J),
            None,
        )
        .is_err());
    }

    #[test]
    fn vi_status_wire_preserves_every_typed_antialias_mode() {
        for (mode, bits) in [
            (fn64_render::ViAaMode::AaResampleAlways, 0),
            (fn64_render::ViAaMode::AaResampleWhenNeeded, 1 << 8),
            (fn64_render::ViAaMode::ResampleOnly, 2 << 8),
            (fn64_render::ViAaMode::Replicate, 3 << 8),
        ] {
            let vi = ViPresentation {
                scanout: ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                    pixel_type: ViPixelType::Rgba16,
                    antialias_mode: mode,
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert_eq!(raw_vi(vi).unwrap().registers[0] & (3 << 8), bits);
        }

        let unspecified = ViPresentation {
            scanout: ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(raw_vi(unspecified).unwrap().registers[0] & (3 << 8), 0);
    }

    #[test]
    fn vi_wire_rejects_rgba32_dither_restoration_by_name() {
        let vi = ViPresentation {
            scanout: ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba32,
                dither_filter: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let error = match validate_native_vi_filters(&vi) {
            Ok(()) => panic!("RGBA32 dither restoration entered native presentation"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            "VI dither restoration requires an RGBA16 scanout image"
        );
    }

    #[test]
    fn vi_wire_preserves_the_complete_noise_seed() {
        for noise_seed in [0, 0x0123_4567_89ab_cdef, u64::MAX] {
            let vi = ViPresentation {
                noise_seed,
                ..Default::default()
            };
            assert_eq!(raw_vi(vi).unwrap().noise_seed, noise_seed);
        }
    }

    #[test]
    fn vi_wire_distinguishes_unspecified_from_hardware_aa_mode_zero() {
        assert_eq!(
            raw_vi(ViPresentation::default()).unwrap().aa_mode_specified,
            0
        );

        let mut words = [0; ViScanoutRegisters::WORD_COUNT];
        words[0] = 2;
        words[2] = 320;
        words[9] = 0x006c_02ec;
        words[10] = 0x0025_01ff;
        let vi = ViPresentation {
            scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
            ..ViPresentation::default()
        };
        let raw = raw_vi(vi).unwrap();
        assert_eq!((raw.registers[0] >> 8) & 3, 0);
        assert_eq!(raw.aa_mode_specified, 1);
    }

    #[test]
    fn cpp_vi_ingress_rejects_registers_without_an_explicit_aa_selector() {
        let task = RawTask::default();
        let mut vi = RawVi {
            registers: [0; 14],
            registers_present: 1,
            blanked: 0,
            fade_enabled: 0,
            repeat_line: 0,
            fade_factor: 0,
            aa_mode_specified: 0,
            reserved: 0,
            noise_seed: 0,
        };
        vi.registers[0] = 2;
        vi.registers[2] = 320;
        vi.registers[9] = 0x006c_02ec;
        vi.registers[10] = 0x0025_01ff;
        let mut capture = RawAdapterCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: every pointer references a live fixed-size C-layout value;
        // the adapter capture retains none of them.
        let ok = unsafe {
            fn64_rt64_capture_adapter_inputs(
                &task,
                0,
                320,
                240,
                &vi,
                &mut capture,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        assert!(error_string(&error, "missing AA-selector diagnostic")
            .contains("requires an explicit AA selector marker"));
    }

    #[test]
    fn cpp_vi_ingress_rejects_an_odd_half_line_extent() {
        let task = RawTask::default();
        let mut vi = RawVi {
            registers: [0; 14],
            registers_present: 1,
            blanked: 0,
            fade_enabled: 0,
            repeat_line: 0,
            fade_factor: 0,
            aa_mode_specified: 1,
            reserved: 0,
            noise_seed: 0,
        };
        vi.registers[0] = 3;
        vi.registers[2] = 320;
        vi.registers[9] = 0x006c_02ec;
        vi.registers[10] = 0x0025_0200;
        let mut capture = RawAdapterCapture::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: every pointer references a live fixed-size C-layout value;
        // the adapter capture retains none of them.
        let ok = unsafe {
            fn64_rt64_capture_adapter_inputs(
                &task,
                0,
                320,
                240,
                &vi,
                &mut capture,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        assert!(error_string(&error, "missing malformed-VI diagnostic")
            .contains("invalid width or active window"));
    }

    fn present_capture_wire(format: u32) -> RawPresentCapture {
        RawPresentCapture {
            width: 3,
            height: 2,
            row_bytes: 12,
            format,
            graphics_api: 2,
            reserved: 0,
            byte_len: 24,
            present_id: 11,
            workload_id: 7,
        }
    }

    #[test]
    fn portable_present_capture_abi_accepts_exact_geometry_format_and_observed_api() {
        for (format_tag, format) in [
            (1, crate::Rt64PresentPixelFormat::Bgra8Unorm),
            (2, crate::Rt64PresentPixelFormat::Rgba8Unorm),
        ] {
            for (api_tag, graphics_api) in [
                (1, ActiveRenderGraphicsApi::D3d12),
                (2, ActiveRenderGraphicsApi::Vulkan),
                (3, ActiveRenderGraphicsApi::Metal),
            ] {
                let mut capture = present_capture_wire(format_tag);
                capture.graphics_api = api_tag;
                assert_eq!(
                    validate_present_capture_metadata(capture).unwrap(),
                    (24, format, graphics_api)
                );
            }
        }
    }

    #[test]
    fn portable_present_capture_abi_rejects_bad_pitch_format_and_provenance() {
        for invalid in [
            RawPresentCapture {
                row_bytes: 16,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                format: 3,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                workload_id: 0,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                graphics_api: 0,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                graphics_api: 4,
                ..present_capture_wire(1)
            },
            RawPresentCapture {
                reserved: 1,
                ..present_capture_wire(1)
            },
        ] {
            assert!(validate_present_capture_metadata(invalid).is_err());
        }
    }

    #[test]
    fn portable_present_capture_keeps_backend_copy_and_fence_seams() {
        let shim = include_str!("../../ffi/fn64_rt64_shim.cpp");
        for required in [
            "minimumLinearTextureAlignmentForPixelFormat",
            "vkCmdCopyImageToBuffer(",
            "D3D12_TEXTURE_DATA_PITCH_ALIGNMENT",
            "d3d_list->d3d->CopyTextureRegion(",
            "present_capture_graphics_api = capture_graphics_api;",
            "capture->graphics_api = context->present_capture_graphics_api;",
            "waitForPresentId(submitted_present);",
            "completed.workloadId",
        ] {
            assert!(
                shim.contains(required),
                "portable present-capture seam lost {required}"
            );
        }
        assert!(
            shim.contains(
                "if (completed.workloadId == 0U) {\n                    // Interleaving closed here: a game's VI thread can present"
            ) && shim.contains("(diagnostic.workload_before != 0U)")
                && shim.contains("(diagnostic.workload_after != 0U)")
                && shim.contains("completed.workloadId > diagnostic.workload_after"),
            "pre-workload VI capture must remain presentable without becoming release evidence"
        );
    }

    #[test]
    fn raster_shader_start_stop_overlay_is_identity_bound_and_shape_checked() {
        let cmake = include_str!("../../ffi/CMakeLists.txt");
        assert!(cmake.contains("FN64_RT64_COMPILATION_THREAD_START_ORIGINAL"));
        assert!(cmake.contains("FN64_RT64_COMPILATION_THREAD_LOOP_ORIGINAL"));
        assert!(cmake.contains("threadRunning = true;\\n        thread ="));
        assert!(cmake.contains("leaves the destructor as its only post-launch writer"));
        assert!(cmake.contains("9b3cf39bb15fc0c7d52085566197042f4960cc410b241e38457bb817f2501e5b"));
        assert!(cmake.contains("fn64_rt64_nominal_full_rate(this)"));
        let expected_overlay = if cfg!(feature = "hfr-evidence") {
            "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3+hfr-post-present-call:v1"
        } else {
            "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3"
        };
        assert_eq!(env!("FN64_RT64_SOURCE_OVERLAY_ID"), expected_overlay);
    }

    #[test]
    fn vi_silhouette_aa_overlay_keeps_wire_selector_and_stage_order_visible() {
        let header = include_str!("../../ffi/fn64_rt64_video_interface.h");
        let shader = include_str!("../../ffi/fn64_rt64_video_interface_ps.hlsl");
        let shim_header = include_str!("../../ffi/fn64_rt64_shim.h");
        let shim = include_str!("../../ffi/fn64_rt64_shim.cpp");
        let cmake = include_str!("../../ffi/CMakeLists.txt");

        assert!(header.contains("uint viFilterFlags;"));
        for flag in [
            "ViFilterDitherRestoration",
            "ViFilterSilhouetteAa",
            "ViFilterRgba16",
            "ViFilterSerratedRows",
        ] {
            assert!(header.contains(flag), "VI filter wire lost {flag}");
        }
        for mechanism in [
            "HasQualifiedPartialCoverage",
            "CoverageAaTexel",
            "CoverageAaNearest",
            "admitted < 3u",
            "(-2, 0)",
            "(2, 0)",
            "float4 center = FilteredTexel(coordinate);",
            "float4 center = FilteredNearest(uv, sourceCoordinate);",
        ] {
            assert!(shader.contains(mechanism), "VI AA shader lost {mechanism}");
        }
        assert!(shim_header.contains("uint8_t aa_mode_specified;"));
        assert!(shim.contains("context.vi_state.aa_mode_specified != 0U"));
        assert!(shim.contains("vi_filter_flags_for_context(*context)"));
        assert!(shim.contains("interop::ViFilterSilhouetteAa"));
        assert!(!cmake.contains("pushConstants.ditherFilter"));
    }

    fn cpp_logical_rate(nominal_refresh_rate: u32, factor: u32) -> Result<u32, String> {
        let mut logical_rate = u32::MAX;
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the scalar output and error buffer remain live for this
        // synchronous, device-free exact-source probe.
        let ok = unsafe {
            fn64_rt64_probe_logical_rate(
                nominal_refresh_rate,
                factor,
                &mut logical_rate,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(&error, "missing logical-rate diagnostic"))
        } else {
            Ok(logical_rate)
        }
    }

    #[test]
    fn cpp_vi_history_uses_context_region_rate_for_stable_factors() {
        assert_eq!(cpp_logical_rate(60, 1).unwrap(), 60);
        assert_eq!(cpp_logical_rate(60, 2).unwrap(), 30);
        assert_eq!(cpp_logical_rate(50, 1).unwrap(), 50);
        assert_eq!(cpp_logical_rate(50, 2).unwrap(), 25);
    }

    #[test]
    fn cpp_vi_history_rejects_missing_or_invalid_region_authority() {
        assert!(cpp_logical_rate(59, 1).unwrap_err().contains("50 or 60 Hz"));
        assert!(cpp_logical_rate(60, 0).unwrap_err().contains("non-zero"));
    }

    #[test]
    fn cpp_vi_history_keeps_concurrent_region_registrations_isolated() {
        let ntsc = std::thread::spawn(|| {
            for _ in 0..128 {
                assert_eq!(cpp_logical_rate(60, 2).unwrap(), 30);
            }
        });
        let pal = std::thread::spawn(|| {
            for _ in 0..128 {
                assert_eq!(cpp_logical_rate(50, 2).unwrap(), 25);
            }
        });
        ntsc.join().unwrap();
        pal.join().unwrap();
    }

    fn raw_roundtrip(input: RawUserConfig) -> Result<RawUserConfig, String> {
        let mut output = RawUserConfig::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: scalar repr(C) input/output and the error buffer are valid
        // for this device-free synchronous validation call.
        let ok = unsafe {
            fn64_rt64_roundtrip_user_config(&input, &mut output, error.as_mut_ptr(), error.len())
        };
        if ok == 0 {
            Err(error_string(&error, "missing settings diagnostic"))
        } else {
            Ok(output)
        }
    }

    #[test]
    fn cpp_settings_validator_accepts_every_public_enum_tag() {
        type RawEnumField = fn(&mut RawUserConfig) -> &mut u32;
        let base = RawUserConfig::from(&RenderRuntimeSettings::default());
        let fields: &[(RawEnumField, u32)] = &[
            (|raw| &mut raw.graphics_api, 4),
            (|raw| &mut raw.resolution, 3),
            (|raw| &mut raw.display_buffering, 2),
            (|raw| &mut raw.antialiasing, 4),
            (|raw| &mut raw.filtering, 3),
            (|raw| &mut raw.aspect_ratio, 3),
            (|raw| &mut raw.extended_aspect_ratio, 3),
            (|raw| &mut raw.upscale_2d, 3),
            (|raw| &mut raw.refresh_rate, 3),
            (|raw| &mut raw.internal_color_format, 3),
            (|raw| &mut raw.hardware_resolve, 3),
        ];
        for (field, count) in fields {
            for tag in 0..*count {
                let mut raw = base;
                *field(&mut raw) = tag;
                assert_eq!(raw_roundtrip(raw).unwrap(), raw);
            }
        }
    }

    #[test]
    fn cpp_settings_validator_rejects_instead_of_clamping_or_coercing() {
        let base = RawUserConfig::from(&RenderRuntimeSettings::default());
        let invalid = [
            RawUserConfig {
                graphics_api: 4,
                ..base
            },
            RawUserConfig {
                three_point_filtering: 2,
                ..base
            },
            RawUserConfig {
                resolution_multiplier: f64::NAN,
                ..base
            },
            RawUserConfig {
                downsample_multiplier: 0,
                ..base
            },
            RawUserConfig {
                aspect_target: 100.1,
                ..base
            },
            RawUserConfig {
                refresh_rate_target: 1001,
                ..base
            },
        ];
        for raw in invalid {
            let error = raw_roundtrip(raw).unwrap_err();
            assert!(
                error.contains("user-config"),
                "unexpected diagnostic: {error}"
            );
        }
    }

    fn raw_enhancement_roundtrip(
        input: RawEnhancementConfig,
    ) -> Result<RawEnhancementConfig, String> {
        let mut output = RawEnhancementConfig::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: scalar repr(C) input/output and the error buffer are valid
        // for this device-free synchronous validation call.
        let ok = unsafe {
            fn64_rt64_roundtrip_enhancement_config(
                &input,
                &mut output,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(&error, "missing enhancement diagnostic"))
        } else {
            Ok(output)
        }
    }

    fn raw_emulator_roundtrip(input: RawEmulatorConfig) -> Result<RawEmulatorConfig, String> {
        let mut output = RawEmulatorConfig::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: scalar repr(C) input/output and the error buffer are valid
        // for this device-free synchronous validation call.
        let ok = unsafe {
            fn64_rt64_roundtrip_emulator_config(
                &input,
                &mut output,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            Err(error_string(&error, "missing emulator diagnostic"))
        } else {
            Ok(output)
        }
    }

    #[test]
    fn cpp_enhancement_and_emulator_validators_reject_unknown_tags_and_booleans() {
        let enhancement = RawEnhancementConfig::from(&RenderEnhancementSettings::default());
        for invalid in [
            RawEnhancementConfig {
                presentation_mode: 3,
                ..enhancement
            },
            RawEnhancementConfig {
                framebuffer_reinterpret_fix_uls: 2,
                ..enhancement
            },
            RawEnhancementConfig {
                s2dex_framebuffer_fast_path: u32::MAX,
                ..enhancement
            },
        ] {
            let error = raw_enhancement_roundtrip(invalid).unwrap_err();
            assert!(
                error.contains("enhancement-config"),
                "unexpected diagnostic: {error}"
            );
        }

        let emulator = RawEmulatorConfig::from(&RenderEmulatorSettings::default());
        for invalid in [
            RawEmulatorConfig {
                post_blend_noise: 2,
                ..emulator
            },
            RawEmulatorConfig {
                framebuffer_render_to_ram: u32::MAX,
                ..emulator
            },
        ] {
            let error = raw_emulator_roundtrip(invalid).unwrap_err();
            assert!(
                error.contains("emulator-config"),
                "unexpected diagnostic: {error}"
            );
        }
    }

    #[test]
    fn stream_worker_evidence_control_rejects_invalid_input_and_missing_setup() {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the validation boundary rejects the invalid scalar before
        // dereferencing the deliberately null context.
        let ok = unsafe {
            fn64_rt64_set_stream_workers_paused(
                std::ptr::null_mut(),
                2,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        let diagnostic = error_string(&error, "missing stream-worker diagnostic");
        assert!(
            diagnostic.contains("stream_workers_paused") && diagnostic.contains("boolean"),
            "unexpected diagnostic: {diagnostic}"
        );

        error.fill(0);
        // SAFETY: a valid scalar with a null context is the public missing-
        // setup error path and retains no pointer.
        let ok = unsafe {
            fn64_rt64_set_stream_workers_paused(
                std::ptr::null_mut(),
                1,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(ok, 0);
        let diagnostic = error_string(&error, "missing stream-worker setup diagnostic");
        assert!(
            diagnostic.contains("requires a completed setup"),
            "unexpected diagnostic: {diagnostic}"
        );
    }

    fn complete_extended_wire() -> RawExtendedGbiEvidence {
        let mut raw = RawExtendedGbiEvidence {
            workload_id: 7,
            present_id: 9,
            enabled_opcode: 0x64,
            hook_enable_count: 1,
            has_refresh_rate: 1,
            refresh_rate: 60,
            rect_count: 1,
            group_count: 1,
            vertex_z_count: 2,
            generated_present_count: 2,
            ..Default::default()
        };
        raw.command_counts[0x06] = 1;
        raw.command_counts[0x09] = 1;
        raw.command_counts[0x0A] = 1;
        raw.command_counts[0x0B] = 1;
        raw.command_counts[0x0C] = 1;
        raw.rects[0] = RawExtendedRectEvidence {
            draw_call_uid: 11,
            left_origin: 0x200,
            right_origin: 0x400,
            left_offset: -4,
            top_offset: 8,
            right_offset: 12,
            bottom_offset: -16,
            upper_left_x: 4,
            upper_left_y: 8,
            lower_right_x: 40,
            lower_right_y: 44,
            aspect_mode: 2,
        };
        raw.groups[0] = RawTransformGroupEvidence {
            group_id: 42,
            projection: 0,
            push: 1,
            decompose: 1,
            editable: 1,
            position_selector: 1,
            rotation_selector: 2,
            scale_selector: 0,
            skew_selector: 0,
            perspective_selector: 1,
            vertex_selector: 1,
            texcoord_selector: 0,
            tile_selector: 2,
            look_at_selector: 1,
            ordering: 1,
            aspect_mode: 2,
            reserved: 0,
        };
        raw.vertex_z[0] = RawVertexZEvidence {
            marker_kind: 1,
            command_vertex_index: 3,
            resolved_source_index: 17,
            affected_face_index_start: 12,
            affected_face_index_count: 6,
        };
        raw.vertex_z[1] = RawVertexZEvidence {
            marker_kind: 2,
            command_vertex_index: u32::MAX,
            resolved_source_index: 17,
            affected_face_index_start: 12,
            affected_face_index_count: 6,
        };
        for (index, generated) in raw.generated_presents.iter_mut().take(2).enumerate() {
            *generated = RawGeneratedPresentEvidence {
                previous_workload_id: 6,
                current_workload_id: 7,
                present_id: 9,
                presentation_ordinal: index as u32,
                interpolation_numerator: index as u32 + 1,
                interpolation_denominator: 2,
                original_refresh_rate: 60,
                target_refresh_rate: 120,
            };
        }
        raw
    }

    #[test]
    fn extended_evidence_wire_decodes_every_semantic_field() {
        let evidence = extended_evidence_from_raw(complete_extended_wire()).unwrap();
        assert_eq!(evidence.workload_id, 7);
        assert_eq!(evidence.present_id, 9);
        assert_eq!(evidence.enabled_opcode, Some(0x64));
        assert_eq!(evidence.refresh_rate, Some(60));
        assert_eq!(evidence.rects[0].left_offset, -4);
        assert_eq!(
            evidence.rects[0].aspect_mode,
            crate::Rt64ExtendedAspectMode::Adjust
        );
        assert_eq!(evidence.groups[0].class, crate::Rt64TransformClass::Model);
        assert_eq!(
            evidence.groups[0].rotation,
            crate::Rt64TransformComponentSelector::Auto
        );
        assert_eq!(evidence.vertex_z[0].command_vertex_index, Some(3));
        assert_eq!(evidence.vertex_z[1].command_vertex_index, None);
        assert_eq!(
            (
                evidence.generated_presents[0].interpolation_numerator,
                evidence.generated_presents[0].interpolation_denominator
            ),
            (1, 2)
        );
    }

    #[test]
    fn extended_evidence_wire_rejects_overflow_and_ambiguous_tags() {
        let mut excess = complete_extended_wire();
        excess.group_count = EXTENDED_MAX_GROUPS as u32 + 1;
        assert!(extended_evidence_from_raw(excess)
            .unwrap_err()
            .contains("exceeds capacity"));

        let mut bad_selector = complete_extended_wire();
        bad_selector.groups[0].position_selector = 3;
        assert!(extended_evidence_from_raw(bad_selector)
            .unwrap_err()
            .contains("selector"));

        let mut bad_fraction = complete_extended_wire();
        bad_fraction.generated_presents[0].interpolation_denominator = 0;
        assert!(extended_evidence_from_raw(bad_fraction)
            .unwrap_err()
            .contains("inconsistent generated-presentation"));
    }

    #[cfg(feature = "hfr-evidence")]
    fn exact_double_hfr_wire() -> RawHfrEvidence {
        let mut raw = RawHfrEvidence {
            previous_workload_id: 6,
            current_workload_id: 7,
            present_id: 9,
            interpolation_framebuffer_identity: 11,
            interpolation_framebuffer_address: 0x20_0000,
            original_refresh_rate: 60,
            target_refresh_rate: 120,
            presentation_count: 2,
            available_interpolated_target_count: 1,
            presented_counter_value: 1,
            ..Default::default()
        };
        for (index, generated) in raw.generated_presents.iter_mut().take(2).enumerate() {
            *generated = RawGeneratedPresentEvidence {
                previous_workload_id: 6,
                current_workload_id: 7,
                present_id: 9,
                presentation_ordinal: index as u32,
                interpolation_numerator: index as u32 + 1,
                interpolation_denominator: 2,
                original_refresh_rate: 60,
                target_refresh_rate: 120,
            };
        }
        raw
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_wire_decodes_original_control_and_exact_double_rate() {
        let hfr = hfr_evidence_from_raw(exact_double_hfr_wire()).unwrap();
        assert_eq!(hfr.presentation_count, 2);
        assert_eq!(hfr.presented_counter_value, 1);
        assert_eq!(
            hfr.presentations
                .iter()
                .map(|present| (
                    present.kind,
                    present.derived_weight_numerator,
                    present.derived_weight_denominator,
                ))
                .collect::<Vec<_>>(),
            vec![
                (crate::Rt64HfrPresentationKind::SpatialIntermediate, 1, 2),
                (crate::Rt64HfrPresentationKind::CurrentEndpoint, 2, 2),
            ]
        );

        let control = RawHfrEvidence {
            target_refresh_rate: 0,
            presentation_count: 1,
            available_interpolated_target_count: 0,
            presented_counter_value: 1,
            generated_presents: Default::default(),
            ..exact_double_hfr_wire()
        };
        assert!(hfr_evidence_from_raw(control)
            .unwrap()
            .presentations
            .is_empty());
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_wire_rejects_counter_identity_and_fraction_drift() {
        let mut wrong_counter = exact_double_hfr_wire();
        wrong_counter.presented_counter_value = 2;
        assert!(hfr_evidence_from_raw(wrong_counter).is_err());

        let mut duplicate_id = exact_double_hfr_wire();
        duplicate_id.previous_workload_id = duplicate_id.current_workload_id;
        assert!(hfr_evidence_from_raw(duplicate_id).is_err());

        let mut wrong_fraction = exact_double_hfr_wire();
        wrong_fraction.generated_presents[0].interpolation_numerator = 2;
        assert!(hfr_evidence_from_raw(wrong_fraction).is_err());
    }

    #[cfg(feature = "hfr-evidence")]
    fn exact_hfr_pacing_wire() -> RawHfrPacingEvidence {
        let mut raw = RawHfrPacingEvidence {
            sample_count: 4,
            ..Default::default()
        };
        for burst in 0..2 {
            for ordinal in 0..2 {
                let index = burst * 2 + ordinal;
                let start = 1_000_000 + index as u64 * 8_333_333;
                raw.samples[index] = RawHfrPacingSample {
                    call_start_monotonic_ns: start,
                    call_return_monotonic_ns: start + 20_000,
                    present_id: 10 + burst as u64,
                    burst_ordinal: ordinal as u32,
                    burst_count: 2,
                    original_refresh_rate: 60,
                    target_refresh_rate: 120,
                    swapchain_valid: 1,
                    reserved: 0,
                };
            }
        }
        raw
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_pacing_wire_decodes_exact_ordered_multi_burst_calls() {
        let pacing = hfr_pacing_from_raw(exact_hfr_pacing_wire()).unwrap();
        assert_eq!(pacing.samples.len(), 4);
        assert_eq!(pacing.samples[0].present_id, 10);
        assert_eq!(pacing.samples[1].burst_ordinal, 1);
        assert_eq!(pacing.samples[2].present_id, 11);
        assert_eq!(
            pacing.samples[3].call_return_monotonic_ns - pacing.samples[3].call_start_monotonic_ns,
            20_000
        );
    }

    #[cfg(feature = "hfr-evidence")]
    #[test]
    fn hfr_pacing_wire_rejects_tail_pair_order_time_and_success_drift() {
        let mut nonempty_tail = exact_hfr_pacing_wire();
        nonempty_tail.samples[4] = nonempty_tail.samples[3];
        assert!(hfr_pacing_from_raw(nonempty_tail).is_err());

        let mut incomplete = exact_hfr_pacing_wire();
        incomplete.sample_count = 3;
        assert!(hfr_pacing_from_raw(incomplete).is_err());

        let mut order_drift = exact_hfr_pacing_wire();
        order_drift.samples[1].present_id = 12;
        assert!(hfr_pacing_from_raw(order_drift).is_err());

        let mut zero_duration = exact_hfr_pacing_wire();
        zero_duration.samples[0].call_return_monotonic_ns =
            zero_duration.samples[0].call_start_monotonic_ns;
        assert!(hfr_pacing_from_raw(zero_duration).is_err());

        let mut invalid_success = exact_hfr_pacing_wire();
        invalid_success.samples[0].swapchain_valid = 2;
        assert!(hfr_pacing_from_raw(invalid_success).is_err());
    }

    #[cfg(feature = "synthetic-f3dex2-evidence")]
    #[test]
    fn synthetic_f3dex2_transport_bounds_the_full_rgba16_target() {
        let shim = include_str!("../../ffi/fn64_rt64_shim.cpp");
        for required in [
            "static_cast<uint64_t>(context->width) * context->height * 2U",
            "static_cast<uint64_t>(rdram_len) - target_byte_len",
            "(output_addr & 0xFF000000U) != 0U",
        ] {
            assert!(
                shim.contains(required),
                "synthetic target guard lost {required}"
            );
        }
    }

    fn generated_capture_wire(ordinal: u32) -> RawExtendedPresentCapture {
        RawExtendedPresentCapture {
            capture_generation: 20 + u64::from(ordinal),
            workload_id: 7,
            present_id: 9,
            capture_ordinal: ordinal,
            capture_count: 2,
            generated_ordinal: ordinal,
            interpolation_numerator: ordinal + 1,
            interpolation_denominator: 2,
            width: 2,
            height: 1,
            row_bytes: 8,
            format: 1,
            byte_len: 8,
        }
    }

    #[test]
    fn extended_present_capture_wire_decodes_exact_pixels_and_fraction() {
        let pixels = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let first =
            extended_present_capture_from_raw(generated_capture_wire(0), pixels.clone()).unwrap();
        let second = extended_present_capture_from_raw(generated_capture_wire(1), pixels).unwrap();
        assert_eq!(first.generated_ordinal, Some(0));
        assert_eq!(second.generated_ordinal, Some(1));
        assert_eq!(first.bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            (
                second.interpolation_numerator,
                second.interpolation_denominator
            ),
            (2, 2)
        );
    }

    #[test]
    fn extended_present_capture_wire_rejects_bad_count_geometry_and_provenance() {
        let bytes = vec![0; 8];
        for invalid in [
            RawExtendedPresentCapture {
                capture_count: 9,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                workload_id: 0,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                generated_ordinal: 1,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                row_bytes: 4,
                ..generated_capture_wire(0)
            },
            RawExtendedPresentCapture {
                format: 99,
                ..generated_capture_wire(0)
            },
        ] {
            assert!(extended_present_capture_from_raw(invalid, bytes.clone()).is_err());
        }
    }

    #[test]
    fn extended_evidence_controls_fail_loudly_without_a_live_context() {
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the public C boundary validates the deliberately null
        // context before dereferencing it and retains no pointer.
        let armed = unsafe {
            fn64_rt64_enable_extended_gbi_evidence(
                std::ptr::null_mut(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(armed, 0);
        assert!(error_string(&error, "missing arm diagnostic").contains("not initialized"));

        error.fill(0);
        let mut evidence = RawExtendedGbiEvidence::default();
        // SAFETY: the public C boundary again validates the null context
        // before touching the live output or retaining either pointer.
        let read = unsafe {
            fn64_rt64_read_extended_gbi_evidence(
                std::ptr::null_mut(),
                &mut evidence,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing read diagnostic").contains("not initialized"));

        error.fill(0);
        let mut capture = RawExtendedPresentCapture::default();
        // SAFETY: the public C boundary rejects the null context before
        // touching either output pointer.
        let read = unsafe {
            fn64_rt64_read_extended_present_capture(
                std::ptr::null_mut(),
                0,
                &mut capture,
                std::ptr::null_mut(),
                0,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing capture diagnostic").contains("not initialized"));
    }

    #[test]
    fn framebuffer_copy_path_evidence_fails_loudly_without_a_live_context() {
        let mut evidence = RawFramebufferCopyPathEvidence::default();
        let mut error = [0; ERROR_CAPACITY];
        // SAFETY: the public C boundary rejects the deliberately null context
        // before touching the fixed-size output or retaining either pointer.
        let read = unsafe {
            fn64_rt64_read_framebuffer_copy_path_evidence(
                std::ptr::null_mut(),
                &mut evidence,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing copy-path diagnostic").contains("not initialized"));
        assert_eq!(evidence, RawFramebufferCopyPathEvidence::default());

        let mut s2dex = RawS2dexFastPathEvidence::default();
        error.fill(0);
        // SAFETY: the same null-context precondition rejects the call before
        // touching the fixed-size S2DEX output or retaining either pointer.
        let read = unsafe {
            fn64_rt64_read_s2dex_fast_path_evidence(
                std::ptr::null_mut(),
                &mut s2dex,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert_eq!(read, 0);
        assert!(error_string(&error, "missing S2DEX diagnostic").contains("not initialized"));
        assert_eq!(s2dex, RawS2dexFastPathEvidence::default());
    }
