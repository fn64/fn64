    use super::*;
    use crate::{F3dzex2Variant, TaskAdmissionUcode};

    const DL: u32 = 0x1000;
    const TEXT: u32 = 0x2000;
    const DATA: u32 = 0x4000;

    fn write_command(rdram: &mut [u8], address: u32, w0: u32, w1: u32) {
        let mut view = RdramViewMut::from_storage(rdram);
        view.write_u32(RdramAddr::from_offset(address), w0);
        view.write_u32(RdramAddr::from_offset(address + 4), w1);
    }

    fn write_logical(rdram: &mut [u8], address: u32, bytes: &[u8]) {
        RdramViewMut::from_storage(rdram)
            .write_logical_bytes(RdramAddr::from_offset(address), bytes);
    }

    fn fixture_for_family(
        family: GeometryWireFamily,
    ) -> (Vec<u8>, RspMemory, OsTask, GeometryUcodeCatalog) {
        let mut rdram = vec![0; 0x9000];
        let text: Vec<u8> = (0..SP_UCODE_SIZE)
            .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
            .collect();
        let data = [0x31, 0x42, 0x53, 0x64, 0x75, 0x86, 0x97, 0xa8];
        write_logical(&mut rdram, TEXT, &text);
        write_logical(&mut rdram, DATA, &data);
        let mut rsp = RspMemory::new();
        rsp.write_bytes(RspMemAddr::from_parts(RspMemoryBank::Imem, 0), &text)
            .unwrap();
        let mut catalog = GeometryUcodeCatalog::default();
        catalog.admit_text_for(family, &text);
        let task = OsTask {
            ucode: TEXT,
            ucode_data: DATA,
            ucode_data_size: data.len() as u32,
            data_ptr: DL,
            ..OsTask::default()
        };
        (rdram, rsp, task, catalog)
    }

    fn fixture() -> (Vec<u8>, RspMemory, OsTask, GeometryUcodeCatalog) {
        fixture_for_family(GeometryWireFamily::F3dex2)
    }

    fn f3dzex2_profile(variant: F3dzex2Variant) -> GeometryUcodeProfile {
        GeometryUcodeProfile::from_admission_ucode(TaskAdmissionUcode::F3dzex2(variant))
            .expect("typed F3DZEX2 admission identity is a geometry profile")
    }

    fn profile_for_wire(family: GeometryWireFamily) -> GeometryUcodeProfile {
        match family {
            GeometryWireFamily::F3dzex2 => f3dzex2_profile(F3dzex2Variant::NoNFifo206H),
            _ => GeometryUcodeProfile::from_public_family(family),
        }
    }

    fn walk_direct_profile(
        profile: GeometryUcodeProfile,
        commands: &[(u32, u32, u32)],
        vertices: &[(usize, ControlVertex)],
        policy: GeometryTaskInspectionPolicy,
    ) -> Result<WalkState, RenderError> {
        let mut rdram = vec![0; 0x9000];
        for &(address, w0, w1) in commands {
            write_command(&mut rdram, address, w0, w1);
        }
        let mut state = WalkState::new(profile);
        for &(slot, vertex) in vertices {
            state.vertices[slot] = vertex;
        }
        walk(
            &mut rdram,
            &mut RspMemory::new(),
            DL,
            &GeometryUcodeCatalog::default(),
            policy,
            None,
            &mut state,
        )?;
        Ok(state)
    }

    fn walk_direct(
        family: GeometryWireFamily,
        commands: &[(u32, u32, u32)],
        vertices: &[(usize, ControlVertex)],
        policy: GeometryTaskInspectionPolicy,
    ) -> Result<WalkState, RenderError> {
        walk_direct_profile(profile_for_wire(family), commands, vertices, policy)
    }

    fn control_vertex(clip_w: f32, z_screen: u32) -> ControlVertex {
        ControlVertex {
            clip_code: 0,
            z_screen,
            clip_w: Some(clip_w),
        }
    }

    fn load_word(bytes: usize) -> u32 {
        (u32::from(G_LOAD_UCODE) << 24) | (bytes as u32 - 1)
    }

    fn dma_word(write_to_dram: bool, rsp_address: u16, bytes: usize) -> u32 {
        (u32::from(G_DMA_IO) << 24)
            | (u32::from(write_to_dram) << 23)
            | ((u32::from(rsp_address) / 8) << 13)
            | (bytes as u32 - 1)
    }

    #[test]
    fn every_supported_public_polygon_family_has_an_explicit_wire_test() {
        let families = [
            GeometryWireFamily::Fast3d,
            GeometryWireFamily::F3dex,
            GeometryWireFamily::F3dlx,
            GeometryWireFamily::F3dlxRej,
            GeometryWireFamily::F3dex2,
            GeometryWireFamily::F3dex2NoN,
            GeometryWireFamily::F3dex2Rej,
            GeometryWireFamily::F3dlx2Rej,
        ];
        for family in families {
            let (mut rdram, rsp, task, catalog) = fixture_for_family(family);
            let end = if is_modern(family) {
                G_ENDDL
            } else {
                LEGACY_G_ENDDL
            };
            write_command(&mut rdram, DL, u32::from(end) << 24, 0);
            let result = inspect_geometry_task(
                &rdram,
                &rsp,
                &task,
                &catalog,
                GeometryTaskInspectionPolicy::default(),
                None,
            )
            .unwrap_or_else(|error| panic!("{} inspection failed: {error}", family.name()));
            assert_eq!(result.admission_plan.entry().family(), family.ucode_id());
        }
    }

    #[test]
    fn line_families_are_named_frontiers() {
        for family in [GeometryWireFamily::L3dex, GeometryWireFamily::L3dex2] {
            let (rdram, rsp, task, catalog) = fixture_for_family(family);
            let error = inspect_geometry_task(
                &rdram,
                &rsp,
                &task,
                &catalog,
                GeometryTaskInspectionPolicy::default(),
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains(family.name()));
            assert!(error.to_string().contains("polygon-family frontier"));
        }
    }

    #[test]
    fn nested_call_tail_and_full_sync_count_follow_executed_path() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_DL) << 24, 0x1100);
        write_command(&mut rdram, DL + 8, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        write_command(
            &mut rdram,
            0x1100,
            (u32::from(G_DL) << 24) | (1 << 16),
            0x1200,
        );
        write_command(&mut rdram, 0x1108, u32::from(G_SPECIAL_1) << 24, 0);
        write_command(&mut rdram, 0x1200, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, 0x1208, u32::from(G_ENDDL) << 24, 0);

        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        assert_eq!(result.full_sync_count, 2);
        assert_eq!(result.dp_full_sync, DpFullSyncStatus::Reached);
    }

    #[test]
    fn forced_depth_branch_changes_both_completion_and_generation_path() {
        let (mut rdram, rsp, task, mut catalog) = fixture();
        const OTHER_TEXT: u32 = 0x5000;
        const OTHER_DATA: u32 = 0x6800;
        const TARGET: u32 = 0x1800;
        let other: Vec<u8> = (0..SP_UCODE_SIZE)
            .map(|index| (index as u8).wrapping_mul(19).wrapping_add(3))
            .collect();
        write_logical(&mut rdram, OTHER_TEXT, &other);
        write_logical(&mut rdram, OTHER_DATA, &[1, 3, 5, 7, 9, 11, 13, 15]);
        catalog.admit_text(&other);
        write_command(&mut rdram, DL, u32::from(G_RDPHALF_1) << 24, TARGET);
        // Slot zero defaults to screen Z zero; threshold u32::MAX makes the
        // ordinary condition true. Invert the fixture by setting Z to max.
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_MODIFYVTX) << 24) | (u32::from(G_MWO_POINT_ZSCREEN) << 16),
            u32::MAX,
        );
        write_command(&mut rdram, DL + 16, u32::from(G_BRANCH_Z) << 24, 0);
        write_command(&mut rdram, DL + 24, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, DL + 32, u32::from(G_ENDDL) << 24, 0);
        write_command(&mut rdram, TARGET, u32::from(G_RDPHALF_1) << 24, OTHER_DATA);
        write_command(&mut rdram, TARGET + 8, load_word(8), OTHER_TEXT);
        write_command(&mut rdram, TARGET + 16, u32::from(G_ENDDL) << 24, 0);

        let normal = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        let forced = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy { force_branch: true },
            None,
        )
        .unwrap();
        assert_eq!(normal.full_sync_count, 1);
        assert_eq!(normal.admission_plan.len(), 1);
        assert_eq!(forced.full_sync_count, 0);
        assert_eq!(forced.admission_plan.len(), 2);
    }

    #[test]
    fn f3dzex2_branch_w_uses_strict_transformed_w_comparison() {
        const TARGET: u32 = 0x1800;
        const SLOT: usize = 5;
        let commands = [
            (DL, u32::from(G_RDPHALF_1) << 24, TARGET),
            (
                DL + 8,
                (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1),
                10,
            ),
            (DL + 16, u32::from(G_RDPFULLSYNC) << 24, 0),
            (DL + 24, u32::from(G_ENDDL) << 24, 0),
            (TARGET, u32::from(G_ENDDL) << 24, 0),
        ];

        for variant in [
            F3dzex2Variant::NoNFifo206H,
            F3dzex2Variant::NoNFifo208I,
            F3dzex2Variant::NoNFifo208J,
        ] {
            for (clip_w, expected_full_syncs) in [(9.0, 0), (10.0, 1), (11.0, 1)] {
                let state = walk_direct_profile(
                    f3dzex2_profile(variant),
                    &commands,
                    &[(SLOT, control_vertex(clip_w, 0))],
                    GeometryTaskInspectionPolicy::default(),
                )
                .unwrap();
                assert_eq!(state.profile.f3dzex2_variant(), Some(variant));
                assert_eq!(
                    state.full_sync_count, expected_full_syncs,
                    "variant={variant:?}, W={clip_w}"
                );
            }
        }
    }

    #[test]
    fn f3dzex2_branch_w_uses_cpp_u32_to_f32_threshold_rounding() {
        const SLOT: usize = 6;
        let state = walk_direct(
            GeometryWireFamily::F3dzex2,
            &[
                (
                    DL,
                    (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1),
                    16_777_217,
                ),
                (DL + 8, u32::from(G_RDPFULLSYNC) << 24, 0),
                (DL + 16, u32::from(G_ENDDL) << 24, 0),
            ],
            &[(SLOT, control_vertex(16_777_216.0, 0))],
            GeometryTaskInspectionPolicy::default(),
        )
        .unwrap();

        assert_eq!(16_777_217_u32 as f32, 16_777_216.0);
        assert_eq!(state.full_sync_count, 1);
    }

    #[test]
    fn f3dzex2_forced_branch_takes_after_validating_the_vertex() {
        const TARGET: u32 = 0x1800;
        const SLOT: usize = 7;
        let state = walk_direct(
            GeometryWireFamily::F3dzex2,
            &[
                (DL, u32::from(G_RDPHALF_1) << 24, TARGET),
                (
                    DL + 8,
                    (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1),
                    10,
                ),
                (DL + 16, u32::from(G_RDPFULLSYNC) << 24, 0),
                (DL + 24, u32::from(G_ENDDL) << 24, 0),
                (TARGET, u32::from(G_ENDDL) << 24, 0),
            ],
            &[(SLOT, control_vertex(11.0, 0))],
            GeometryTaskInspectionPolicy { force_branch: true },
        )
        .unwrap();
        assert_eq!(state.full_sync_count, 0);
    }

    #[test]
    fn f3dzex2_branch_w_ignores_bits_23_through_8_and_bit_zero() {
        const TARGET: u32 = 0x1800;
        const SLOT: usize = 5;
        let state = walk_direct(
            GeometryWireFamily::F3dzex2,
            &[
                (DL, u32::from(G_RDPHALF_1) << 24, TARGET),
                (
                    DL + 8,
                    (u32::from(G_BRANCH_Z) << 24) | 0x00ff_ff00 | ((SLOT as u32) << 1) | 1,
                    10,
                ),
                (DL + 16, u32::from(G_RDPFULLSYNC) << 24, 0),
                (DL + 24, u32::from(G_ENDDL) << 24, 0),
                (TARGET, u32::from(G_ENDDL) << 24, 0),
            ],
            &[(SLOT, control_vertex(9.0, 0))],
            GeometryTaskInspectionPolicy::default(),
        )
        .unwrap();
        assert_eq!(state.full_sync_count, 0);
    }

    #[test]
    fn f3dex2_branch_z_and_f3dzex2_branch_w_keep_opposite_fixture_paths() {
        const TARGET: u32 = 0x1800;
        const SLOT: usize = 1;
        let branch = (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32 * 5) << 12) | (SLOT as u32 * 2);
        let commands = [
            (DL, u32::from(G_RDPHALF_1) << 24, TARGET),
            (DL + 8, branch, 15),
            (DL + 16, u32::from(G_RDPFULLSYNC) << 24, 0),
            (DL + 24, u32::from(G_ENDDL) << 24, 0),
            (TARGET, u32::from(G_ENDDL) << 24, 0),
        ];
        let vertex = control_vertex(10.0, 20);

        let branch_z = walk_direct(
            GeometryWireFamily::F3dex2,
            &commands,
            &[(SLOT, vertex)],
            GeometryTaskInspectionPolicy::default(),
        )
        .unwrap();
        let branch_w = walk_direct(
            GeometryWireFamily::F3dzex2,
            &commands,
            &[(SLOT, vertex)],
            GeometryTaskInspectionPolicy::default(),
        )
        .unwrap();

        assert_eq!(branch_z.full_sync_count, 1);
        assert_eq!(branch_w.full_sync_count, 0);
    }

    #[test]
    fn f3dzex2_branch_w_requires_half_1_only_on_a_taken_path() {
        const SLOT: usize = 3;
        let commands = [
            (DL, (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1), 10),
            (DL + 8, u32::from(G_RDPFULLSYNC) << 24, 0),
            (DL + 16, u32::from(G_ENDDL) << 24, 0),
        ];

        let error = walk_direct(
            GeometryWireFamily::F3dzex2,
            &commands,
            &[(SLOT, control_vertex(9.0, 0))],
            GeometryTaskInspectionPolicy::default(),
        )
        .err()
        .expect("taken BranchW without HALF_1 must fail");
        assert!(error.to_string().contains("G_BRANCH_W"));
        assert!(error.to_string().contains("G_RDPHALF_1"));

        let state = walk_direct(
            GeometryWireFamily::F3dzex2,
            &commands,
            &[(SLOT, control_vertex(10.0, 0))],
            GeometryTaskInspectionPolicy::default(),
        )
        .unwrap();
        assert_eq!(state.full_sync_count, 1);
    }

    #[test]
    fn f3dzex2_branch_w_rejects_unloaded_and_non_finite_vertices() {
        const SLOT: usize = 127;
        let commands = [(DL, (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1), 10)];

        let unloaded = walk_direct(
            GeometryWireFamily::F3dzex2,
            &commands,
            &[],
            GeometryTaskInspectionPolicy::default(),
        )
        .err()
        .expect("unloaded BranchW vertex must fail");
        assert!(unloaded.to_string().contains("unloaded cache slot 127"));

        let non_finite = walk_direct(
            GeometryWireFamily::F3dzex2,
            &commands,
            &[(SLOT, control_vertex(f32::NAN, 0))],
            GeometryTaskInspectionPolicy { force_branch: true },
        )
        .err()
        .expect("non-finite BranchW vertex must fail");
        assert!(non_finite.to_string().contains("non-finite transformed W"));
    }

    #[test]
    fn f3dzex2_branch_w_resolves_and_masks_segmented_tail_targets() {
        const SLOT: usize = 2;
        let state = walk_direct(
            GeometryWireFamily::F3dzex2,
            &[
                (
                    DL,
                    (u32::from(G_MOVEWORD) << 24) | (u32::from(G_MW_SEGMENT) << 16) | 12,
                    0x1003,
                ),
                (DL + 8, u32::from(G_RDPHALF_1) << 24, 0x0300_080a),
                (
                    DL + 16,
                    (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1),
                    10,
                ),
                (DL + 24, u32::from(G_ENDDL) << 24, 0),
                (0x1808, u32::from(G_RDPFULLSYNC) << 24, 0),
                (0x1810, u32::from(G_ENDDL) << 24, 0),
            ],
            &[(SLOT, control_vertex(9.0, 0))],
            GeometryTaskInspectionPolicy::default(),
        )
        .unwrap();
        assert_eq!(state.full_sync_count, 1);
    }

    #[test]
    fn moveword_g_mw_matrix_is_a_named_unsupported_frontier() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(
            &mut rdram,
            DL,
            (u32::from(G_MOVEWORD) << 24) | (u32::from(G_MW_MATRIX) << 16),
            0,
        );
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("G_MW_MATRIX"));
        assert!(error.to_string().contains("fixed-point matrix patch"));
    }

    #[test]
    fn f3dzex2_branch_w_rejects_an_invalid_taken_target_immediately() {
        const SLOT: usize = 4;
        let error = walk_direct(
            GeometryWireFamily::F3dzex2,
            &[
                (DL, u32::from(G_RDPHALF_1) << 24, 0x00ff_fff8),
                (
                    DL + 8,
                    (u32::from(G_BRANCH_Z) << 24) | ((SLOT as u32) << 1),
                    10,
                ),
            ],
            &[(SLOT, control_vertex(9.0, 0))],
            GeometryTaskInspectionPolicy::default(),
        )
        .err()
        .expect("invalid BranchW target must fail");
        assert!(error.to_string().contains("G_BRANCH_W target"));
        assert!(error.to_string().contains("exceeds RDRAM length"));
    }

    #[test]
    fn z_screen_modify_does_not_replace_f3dzex2_transformed_w() {
        const SLOT: usize = 9;
        let mut state = WalkState::new(f3dzex2_profile(F3dzex2Variant::NoNFifo206H));
        state.vertices[SLOT] = control_vertex(12.25, 3);
        modify_vertex(
            (u32::from(G_MODIFYVTX) << 24)
                | (u32::from(G_MWO_POINT_ZSCREEN) << 16)
                | (SLOT as u32 * 2),
            u32::MAX,
            &mut state,
        )
        .unwrap();
        assert_eq!(state.vertices[SLOT].z_screen, u32::MAX);
        assert_eq!(state.vertices[SLOT].clip_w, Some(12.25));
    }

    #[test]
    fn projected_vertex_retains_homogeneous_w_for_f3dzex2_branching() {
        let mut state = WalkState::new(f3dzex2_profile(F3dzex2Variant::NoNFifo208J));
        let mut matrix = identity();
        matrix[3][3] = 2.0;
        state.mvp = Some(matrix);
        state.viewport = Some(Viewport { sz: 1.0, tz: 0.0 });

        let vertex = project_vertex(&state, 1.0, 2.0, 3.0).unwrap();
        assert_eq!(vertex.clip_w, Some(2.0));

        state.mvp = None;
        let identity_vertex = project_vertex(&state, 1.0, 2.0, 3.0).unwrap();
        assert_eq!(identity_vertex.clip_w, Some(1.0));

        state.persp_normalize = Some(0);
        let collapsed = project_vertex(&state, 1.0, 2.0, 3.0).unwrap();
        assert_eq!(collapsed.clip_w, Some(0.0));
    }

    #[test]
    fn cull_uses_transformed_clip_codes_to_skip_a_self_load() {
        let (mut rdram, rsp, task, mut catalog) = fixture();
        const MATRIX: u32 = 0x7000;
        const VIEWPORT: u32 = 0x7100;
        const VERTICES: u32 = 0x7200;
        const OTHER_TEXT: u32 = 0x5000;
        const OTHER_DATA: u32 = 0x6800;
        let other = vec![0x5d; SP_UCODE_SIZE];
        write_logical(&mut rdram, OTHER_TEXT, &other);
        write_logical(&mut rdram, OTHER_DATA, &[2, 4, 6, 8, 10, 12, 14, 16]);
        catalog.admit_text(&other);
        // Identity matrix in split 16.16 Mtx layout.
        for index in 0..4 {
            RdramViewMut::from_storage(&mut rdram)
                .write_u16(RdramAddr::from_offset(MATRIX + (index * 10) as u32), 1);
        }
        // Positive Z viewport scale/translate, only needed to satisfy the
        // transformed-vertex contract.
        RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(VIEWPORT + 4), 4);
        RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(VIEWPORT + 12), 4);
        for index in 0..2 {
            let base = VERTICES + index * VTX_STRIDE as u32;
            RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(base), 2);
        }
        write_command(
            &mut rdram,
            DL,
            (u32::from(G_MOVEMEM) << 24) | (1 << 19) | u32::from(G_MV_VIEWPORT),
            VIEWPORT,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_MTX) << 24) | (7 << 19) | 3,
            MATRIX,
        );
        write_command(
            &mut rdram,
            DL + 16,
            (u32::from(G_VTX) << 24) | (2 << 12) | (2 << 1),
            VERTICES,
        );
        write_command(&mut rdram, DL + 24, u32::from(G_CULLDL) << 24, 2);
        write_command(
            &mut rdram,
            DL + 32,
            u32::from(G_RDPHALF_1) << 24,
            OTHER_DATA,
        );
        write_command(&mut rdram, DL + 40, load_word(8), OTHER_TEXT);
        write_command(&mut rdram, DL + 48, u32::from(G_ENDDL) << 24, 0);

        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        assert_eq!(result.admission_plan.len(), 1);
    }

    #[test]
    fn dma_self_modification_preserves_non_palindromic_a_b_a_raw_windows() {
        let (mut rdram, rsp, task, mut catalog) = fixture();
        const PREFIX_A: u32 = 0x7400;
        const PREFIX_B: u32 = 0x7410;
        let a = logical_bytes(&rdram, TEXT, SP_UCODE_SIZE).unwrap();
        let mut b = a.clone();
        b[..8].copy_from_slice(&[0x02, 0x17, 0x2c, 0x41, 0x56, 0x6b, 0x80, 0x95]);
        write_logical(&mut rdram, PREFIX_A, &a[..8]);
        write_logical(&mut rdram, PREFIX_B, &b[..8]);
        catalog.admit_text(&b);
        let mut pc = DL;
        let emit = |rdram: &mut [u8], pc: &mut u32, w0, w1| {
            write_command(rdram, *pc, w0, w1);
            *pc += 8;
        };
        emit(&mut rdram, &mut pc, u32::from(G_RDPHALF_1) << 24, DATA);
        emit(&mut rdram, &mut pc, load_word(8), TEXT);
        emit(&mut rdram, &mut pc, dma_word(false, 0, 8), PREFIX_B);
        emit(&mut rdram, &mut pc, dma_word(true, 0, 8), TEXT);
        emit(&mut rdram, &mut pc, u32::from(G_RDPHALF_1) << 24, DATA);
        emit(&mut rdram, &mut pc, load_word(8), TEXT);
        emit(&mut rdram, &mut pc, dma_word(false, 0, 8), PREFIX_A);
        emit(&mut rdram, &mut pc, dma_word(true, 0, 8), TEXT);
        emit(&mut rdram, &mut pc, u32::from(G_RDPHALF_1) << 24, DATA);
        emit(&mut rdram, &mut pc, load_word(8), TEXT);
        emit(&mut rdram, &mut pc, u32::from(G_ENDDL) << 24, 0);

        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            Some(TaskAdmissionRawWindowSize { text: 16, data: 8 }),
        )
        .unwrap();
        assert_eq!(result.admission_plan.len(), 4);
        assert_eq!(result.raw_windows.len(), 4);
        assert_eq!(result.raw_windows[1], result.raw_windows[3]);
        assert_ne!(result.raw_windows[1], result.raw_windows[2]);
        assert_eq!(
            result.admission_plan.self_loads()[0].text_sha256,
            UcodeDigest::from_text(&a)
        );
        assert_eq!(
            result.admission_plan.self_loads()[1].text_sha256,
            UcodeDigest::from_text(&b)
        );
        assert_eq!(
            result.admission_plan.self_loads()[2].text_sha256,
            UcodeDigest::from_text(&a)
        );
        assert_eq!(
            &rdram[TEXT as usize..TEXT as usize + 8],
            &result.raw_windows[0].text[..8]
        );
    }

    #[test]
    fn texture_rectangle_payload_cannot_fabricate_full_sync() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_TEXRECT) << 24, 0);
        write_command(&mut rdram, DL + 8, u32::from(G_RDPFULLSYNC) << 24, 0);
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        let result = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap();
        assert_eq!(result.full_sync_count, 0);
    }

    #[test]
    fn rejection_records_one_stable_detailed_journal_event() {
        let path = std::env::temp_dir().join(format!(
            "fn64-geometry-inspection-unsupported-{}.journal",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        fn64_runtime::arm_unsupported_events(Some(&path)).unwrap();

        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_SPECIAL_1) << 24, 0x1234_5678);
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        let reason = error.to_string();
        assert!(reason.contains("reserved F3DEX2 command 0xd5"));
        assert!(reason.contains("RDRAM 0x00001000"));

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.subsystem, fn64_runtime::UnsupportedSubsystem::Render);
        assert_eq!(event.operation, "render.geometry-task-inspection.rejected");
        assert_eq!(
            event.disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert_eq!(event.guest_cycle, None);
        assert_eq!(
            event.context,
            "inspector=geometry-task-inspection; reason=reserved F3DEX2 command 0xd5 at RDRAM 0x00001000"
        );

        let journal = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = journal.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "fn64.unsupported-journal.v2\tarmed");
        let fields: Vec<_> = lines[1].split('\t').collect();
        assert_eq!(fields.len(), 8);
        assert_eq!(fields[0], "fn64.unsupported-journal.v2");
        assert_eq!(fields[1], "event");
        assert_eq!(fields[3], "unknown");
        assert_eq!(fields[4], "render");
        assert_eq!(fields[5], "returned_error");
        assert_eq!(
            fields[6],
            "72656e6465722e67656f6d657472792d7461736b2d696e7370656374696f6e2e72656a6563746564"
        );
        assert_eq!(
            fields[7],
            "696e73706563746f723d67656f6d657472792d7461736b2d696e7370656374696f6e3b20726561736f6e3d72657365727665642046334445583220636f6d6d616e64203078643520617420524452414d2030783030303031303030"
        );

        fn64_runtime::arm_unsupported_events(None).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_rejection_does_not_poison_unsupported_evidence() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, (u32::from(G_BRANCH_Z) << 24) | 1, 0);
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("G_BRANCH_Z malformed cache offsets"));
        assert!(fn64_runtime::copy_unsupported_events().is_empty());
    }

    #[test]
    fn needs_lle_is_not_misreported_as_an_inspection_rejection() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let (rdram, mut rsp, task, catalog) = fixture();
        rsp.write_bytes(
            RspMemAddr::from_parts(RspMemoryBank::Imem, 0),
            &[0xa5; SP_UCODE_SIZE],
        )
        .unwrap();

        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, RenderError::RequiresLle { .. }));
        assert!(fn64_runtime::copy_unsupported_events().is_empty());

        const OTHER_TEXT: u32 = 0x5000;
        const OTHER_DATA: u32 = 0x6800;
        let (mut rdram, rsp, task, catalog) = fixture();
        let other = vec![0x5d; SP_UCODE_SIZE];
        write_logical(&mut rdram, OTHER_TEXT, &other);
        write_logical(
            &mut rdram,
            OTHER_DATA,
            &[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
        );
        write_command(&mut rdram, DL, u32::from(G_RDPHALF_1) << 24, OTHER_DATA);
        write_command(&mut rdram, DL + 8, load_word(8), OTHER_TEXT);
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        let RenderError::RequiresLle { ucode_sha256 } = error else {
            panic!("unadmitted self-loaded ucode did not request LLE")
        };
        assert_eq!(ucode_sha256, UcodeDigest::from_text(&other).as_bytes());
        assert!(fn64_runtime::copy_unsupported_events().is_empty());
    }

    #[test]
    fn failure_is_transactional_and_named() {
        let (mut rdram, rsp, task, catalog) = fixture();
        write_command(&mut rdram, DL, u32::from(G_SPECIAL_1) << 24, 0);
        let before_rdram = rdram.clone();
        let before_rsp = rsp.clone();
        let error = inspect_geometry_task(
            &rdram,
            &rsp,
            &task,
            &catalog,
            GeometryTaskInspectionPolicy::default(),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains(INSPECTOR));
        assert_eq!(rdram, before_rdram);
        assert_eq!(rsp, before_rsp);
    }
