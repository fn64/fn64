use super::*;

#[test]
fn exported_private_series_matrix_path_admits_public_fixture_and_rejects_tamper() {
    const REPORT_SCENARIO: &str = "public-homebrew-production-matrix-mechanism-v1";
    const CHILD_FIXTURE: &str = "private_release_series::tests::fresh_child_fixture";
    const CHILD_ENABLE_ENV: &str = "FN64_TEST_RELEASE_CHILD";
    const CHILD_TEMPLATE_ENV: &str = "FN64_TEST_RELEASE_TEMPLATE";

    let directory = ProductionMatrixFixtureDirectory::new();
    let manifest_path = directory.0.join("admission-manifest.json");
    let readiness_path = directory.0.join("readiness.json");
    let contract_path = directory.0.join("contract.json");
    let receipt_path = directory.0.join("program-build-receipt.json");
    let report_template_path = directory.0.join("report-template.json");
    let rom_path = directory.0.join("public-homebrew-fixture.z64");
    let text_path = directory.0.join("microcode-text.bin");
    let data_path = directory.0.join("microcode-data.bin");
    let recompiled_path = directory.0.join("typed-block-pack.bin");
    let series_path = directory.0.join("series");

    // This generated file is a public, non-game homebrew-shaped fixture.
    // It tests the production authority path; it is not representative-ROM
    // or runtime/microcode behavioral evidence.
    let mut rom_bytes = vec![0u8; 0x1000];
    rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    rom_bytes[0x20..0x34].copy_from_slice(b"FN64 MATRIX FIXTURE ");
    rom_bytes[0x3b..0x3f].copy_from_slice(b"NF6E");
    fs::write(&rom_path, &rom_bytes).unwrap();
    fs::write(&text_path, vec![0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]).unwrap();
    fs::write(&data_path, b"fn64 public matrix fixture task data").unwrap();
    fs::write(
        &recompiled_path,
        b"fn64 public matrix fixture typed-block pack",
    )
    .unwrap();

    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let program_sha256 = hex(&Sha256::digest(
        b"fn64 public matrix fixture typed-block program v1",
    ));
    let materialized = materialize_release_program_build_receipt(
        &receipt_path,
        &executable,
        ReleaseProgramBuildReceiptInput::TypedBlock {
            pack: recompiled_path.clone(),
            expected_program_sha256: program_sha256,
        },
    )
    .unwrap();
    let source = materialized.execution_source;
    let recompiled_sha256 = hex(&Sha256::digest(fs::read(&recompiled_path).unwrap()));
    let text_sha256 = hex(&Sha256::digest(fs::read(&text_path).unwrap()));
    let data_bytes = fs::metadata(&data_path).unwrap().len() as u32;
    let data_sha256 = hex(&Sha256::digest(fs::read(&data_path).unwrap()));

    let mut report = closed_report(
        REPORT_SCENARIO,
        &rom_bytes,
        0xd6,
        "save.sram-operation",
        CLEAN_RT64_IDENTITY,
        Some(ProgramFeature::TypedBlock),
    );
    #[cfg(target_os = "macos")]
    let (platform, platform_wire) = (ReleaseHostPlatform::MacosArm64, "macos_arm64");
    #[cfg(target_os = "linux")]
    let (platform, platform_wire) = (ReleaseHostPlatform::LinuxX86_64, "linux_x86_64");
    #[cfg(target_os = "windows")]
    let (platform, platform_wire) = (ReleaseHostPlatform::WindowsX86_64, "windows_x86_64");
    report.environment.platform = platform;
    report.environment.windows_version = crate::test_release_windows_version();
    report.execution_destinations = ExecutionDestinationEvidence::from_ordered(
        source.clone(),
        vec![crate::ExecutionDestinationEventEvidence {
            guest_cycle: None,
            destination: crate::ReleaseExecutionDestination::TypedBlock {
                bank: 1,
                pc: 0x8000_1000,
                runner_artifact_sha256: recompiled_sha256.clone(),
            },
        }],
    )
    .unwrap();
    report.rom = Some(
        ReleaseRomEvidence::from_bytes(
            &rom_bytes,
            ReleaseRomClass::PublicHomebrew,
            fn64_runtime::TvType::Ntsc,
        )
        .unwrap(),
    );
    report.rsp_rdp = RspRdpEvidence::from_ordered(vec![RspRdpObservationEventEvidence {
        guest_cycle: 42,
        observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
            task_address: 0x1000,
            imem_generation: 1,
            text_sha256,
            data_address: 0x2000,
            data_bytes,
            data_sha256,
            family: Some(ReleaseMicrocodeFamily::Other { id: 0x464e_3634 }),
        },
    }])
    .unwrap();
    report.report_sha256 = hex(&Sha256::digest(
        crate::release_gate::encode_report_evidence(&report).unwrap(),
    ));
    report.verify_integrity().unwrap();
    report.write_json(&report_template_path).unwrap();

    let descriptor = |path: &Path, provenance: &str| {
        let bytes = fs::read(path).unwrap();
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "length": bytes.len(),
            "sha256": hex(&Sha256::digest(&bytes)),
            "provenance": provenance,
            "git_identity": "excluded",
        })
    };
    let file_descriptor = |path: &Path| {
        let bytes = fs::read(path).unwrap();
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "length": bytes.len(),
            "sha256": hex(&Sha256::digest(&bytes)),
            "git_identity": "excluded",
        })
    };
    let execution_source = serde_json::to_value(&source).unwrap();
    let manifest = serde_json::json!({
        "schema": "fn64.private-input-admission.v7",
        "purpose": "full_rom",
        "intent": {
            "wire_family": "full_rom_mixed",
            "report_scenario": REPORT_SCENARIO,
            "recognition": "runtime_must_confirm_backend_known_pair",
            "extended_gbi_cases": [],
            "characterization_suite": null,
            "program_evidence_lane": "typed_block_program",
            "rom_class": "public_homebrew",
        },
        "release_matrix": {
            "platform": platform_wire,
            "controllers": ["standard_controller"],
            "save": "sram_32_kib",
            "renderers": ["reference_lle_accuracy"],
            "repeat_bar": 10,
        },
        "artifacts": {
            "microcode_text": descriptor(&text_path, "user_owned_rom_derived"),
            "microcode_data": descriptor(&data_path, "user_owned_rom_derived"),
            "microcode_text_raw_window": null,
            "microcode_data_raw_window": null,
            "rom": descriptor(&rom_path, "publicly_distributed_homebrew_rom"),
            "recompiled": descriptor(&recompiled_path, "user_generated_from_owned_rom"),
        },
        "runner": {
            "executable": file_descriptor(&executable),
            "working_directory": directory.0.to_str().unwrap(),
            "argv": ["--exact", CHILD_FIXTURE, "--nocapture"],
            "env": {
                CHILD_ENABLE_ENV: "1",
                CHILD_TEMPLATE_ENV: report_template_path.to_str().unwrap(),
            },
            "release_gate_cycle": report.digest.guest_cycle,
            "execution_source": execution_source,
            "program_build_receipt": file_descriptor(&receipt_path),
        },
    });
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let admitted = crate::private_input_admission::admit_current_v7_manifest(
        &manifest_path,
        &readiness_path,
    )
    .unwrap();
    assert!(admitted.contract.is_some());
    let write_new = |path: &Path, payload: &[u8]| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.write_all(payload).unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
    };
    write_new(&readiness_path, &admitted.readiness_bytes);
    write_new(
        &contract_path,
        admitted
            .contract_bytes
            .as_deref()
            .expect("full-ROM admission emits a contract"),
    );

    let contract = load_private_release_run_contract(&contract_path).unwrap();
    let receipt = run_private_release_series(&contract, &series_path).unwrap();
    let verified_series =
        verify_private_release_series(&contract, &series_path, &receipt).unwrap();
    let evidence = (1..=RELEASE_MATRIX_REPORT_COUNT)
        .map(|ordinal| {
            let report_path = series_path.join(format!("report-{ordinal:02}.json"));
            let retained_report =
                serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
            let journal_path = report_path.with_extension("unsupported.jsonl");
            let journal = parse_unsupported_journal(&fs::read(journal_path).unwrap()).unwrap();
            (retained_report, journal)
        })
        .collect::<Vec<_>>();
    let matrix_manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("public-homebrew-production-fixture", &report)],
    };

    let ReleaseMatrixVerification::Incomplete(incomplete) =
        verify_release_matrix_with_private_series(
            &matrix_manifest,
            &evidence,
            &[&verified_series],
        )
        .unwrap()
    else {
        panic!("one public fixture must remain incomplete against the full profile")
    };
    let homebrew_assignment = incomplete
        .satisfied
        .iter()
        .find(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::RomClass
                && assignment.requirement.id() == "public_homebrew"
        })
        .expect("production opaque series earns its exact public-homebrew fixture row");
    assert_eq!(homebrew_assignment.evidence_sha256s.len(), 1);

    let mut reordered = evidence.clone();
    let mut journals = reordered
        .iter()
        .map(|(_, journal)| journal.clone())
        .collect::<Vec<_>>();
    journals.rotate_left(1);
    for ((_, journal), replacement) in reordered.iter_mut().zip(journals) {
        *journal = replacement;
    }
    assert!(matches!(
        verify_release_matrix_with_private_series(
            &matrix_manifest,
            &reordered,
            &[&verified_series],
        ),
        Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            field: "run_event_sha256s",
            ..
        })
    ));

    fs::OpenOptions::new()
        .append(true)
        .open(series_path.join("report-01.json"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(matches!(
        verify_release_matrix_with_private_series(
            &matrix_manifest,
            &evidence,
            &[&verified_series],
        ),
        Err(ReleaseMatrixError::InvalidPrivateSeriesAuthority { .. })
    ));
}

#[test]
fn valid_v5_evidence_returns_canonical_incomplete_profile() {
    let (manifest, evidence, incomplete) = incomplete_fixture();
    assert_eq!(FullParityV1::REQUIREMENT_COUNT, 162);
    assert_eq!(incomplete.verified_scenarios, 2);
    assert_eq!(incomplete.verified_reports, 20);
    assert_eq!(incomplete.satisfied.len(), 7);
    assert_eq!(incomplete.missing.len(), 155);
    assert_eq!(
        incomplete.manifest_sha256,
        manifest.recompute_manifest_sha256()
    );

    let satisfied_keys: BTreeSet<_> = incomplete
        .satisfied
        .iter()
        .map(|assignment| {
            (
                assignment.requirement.class(),
                assignment.requirement.id().to_owned(),
            )
        })
        .collect();
    let expected_satisfied: Vec<_> = profile_keys()
        .into_iter()
        .filter(|key| satisfied_keys.contains(key))
        .collect();
    let actual_satisfied: Vec<_> = incomplete
        .satisfied
        .iter()
        .map(|assignment| {
            (
                assignment.requirement.class(),
                assignment.requirement.id().to_owned(),
            )
        })
        .collect();
    assert_eq!(actual_satisfied, expected_satisfied);

    let missing_keys = requirement_keys(incomplete.missing.clone());
    let expected_missing: Vec<_> = profile_keys()
        .into_iter()
        .filter(|key| !satisfied_keys.contains(key))
        .collect();
    assert_eq!(missing_keys, expected_missing);

    let standard = incomplete
        .satisfied
        .iter()
        .find(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::Controller
                && assignment.requirement.id() == "standard_controller"
        })
        .unwrap();
    assert_eq!(standard.evidence_sha256s.len(), 2);
    assert!(satisfied_keys.contains(&(
        CertificationRequirementClass::PlatformApiTarget,
        "linux-vulkan".to_owned(),
    )));

    // Re-running the interleaved flat stream is deterministic.
    let rerun = match verify_release_matrix(&manifest, &evidence).unwrap() {
        ReleaseMatrixVerification::Incomplete(value) => value,
        ReleaseMatrixVerification::Complete(_) => unreachable!(),
    };
    assert_eq!(rerun.assessment_sha256, incomplete.assessment_sha256);
}

#[test]
fn schema_v20_tv_region_credit_requires_fixed_header_evidence_not_labels_or_region_free() {
    let fixed = with_rom(
        closed_report(
            "fixed-ntsc",
            b"placeholder",
            0xd1,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        ),
        b'E',
        crate::ReleaseRomClass::RetailCartridge,
    );
    assert_eq!(
        derive_scenario_coverage("fixed-ntsc", &fixed)
            .unwrap()
            .tv_regions,
        vec![ReleaseTvRegion::Ntsc]
    );
    let fixed_incomplete = incomplete_for_report("fixed-ntsc", fixed);
    assert_eq!(
        assigned_requirement_ids(&fixed_incomplete, CertificationRequirementClass::TvRegion,),
        BTreeSet::from(["ntsc".to_owned()])
    );
    assert!(assigned_requirement_ids(
        &fixed_incomplete,
        CertificationRequirementClass::RomClass,
    )
    .is_empty());

    let region_free = with_rom(
        closed_report(
            "retail-pal-label",
            b"placeholder",
            0xd2,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        ),
        0,
        crate::ReleaseRomClass::PublicHomebrew,
    );
    assert!(derive_scenario_coverage("retail-pal-label", &region_free)
        .unwrap()
        .tv_regions
        .is_empty());
    let region_free_incomplete = incomplete_for_report("retail-pal-label", region_free);
    assert!(assigned_requirement_ids(
        &region_free_incomplete,
        CertificationRequirementClass::TvRegion,
    )
    .is_empty());
    assert!(assigned_requirement_ids(
        &region_free_incomplete,
        CertificationRequirementClass::RomClass,
    )
    .is_empty());

    let label_only = incomplete_for_report(
        "retail-pal-label",
        closed_report(
            "retail-pal-label",
            b"placeholder",
            0xd3,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        ),
    );
    assert!(
        assigned_requirement_ids(&label_only, CertificationRequirementClass::TvRegion,)
            .is_empty()
    );
    assert!(
        assigned_requirement_ids(&label_only, CertificationRequirementClass::RomClass,)
            .is_empty()
    );
}

#[test]
fn rom_class_credit_requires_exact_contract_authority_and_binds_its_digest() {
    let report = with_rom(
        closed_report(
            "authority-retail",
            b"placeholder",
            0xd4,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        ),
        b'E',
        ReleaseRomClass::RetailCartridge,
    );
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("authority-retail", &report)],
    };
    let authority = rom_class_authority(&report);
    let authority_sha256 = authority.authority_sha256.clone();
    let authorities = BTreeMap::from([(report.scenario.clone(), authority)]);
    let ReleaseMatrixVerification::Incomplete(incomplete) =
        verify_release_matrix_with_authorities(
            &manifest,
            &evidence_series(report.clone()),
            &authorities,
            &BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!("one authority-backed series remains intentionally incomplete")
    };
    let assignment = incomplete
        .satisfied
        .iter()
        .find(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::RomClass
                && assignment.requirement.id() == "retail_cartridge"
        })
        .expect("retail class receives authority-backed credit");
    assert_eq!(assignment.evidence_sha256s, [authority_sha256]);

    let mut relabelled = rom_class_authority(&report);
    relabelled.rom_class = ReleaseRomClass::PublicHomebrew;
    relabelled.authority_sha256 = relabelled.recompute_authority_sha256();
    let relabelled = BTreeMap::from([(report.scenario.clone(), relabelled)]);
    assert!(matches!(
        verify_release_matrix_with_authorities(
            &manifest,
            &evidence_series(report.clone()),
            &relabelled,
            &BTreeMap::new(),
        ),
        Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            field: "rom.class",
            ..
        })
    ));

    let mut tampered = rom_class_authority(&report);
    tampered.input_bytes += 4;
    let tampered = BTreeMap::from([(report.scenario.clone(), tampered)]);
    assert!(matches!(
        verify_release_matrix_with_authorities(
            &manifest,
            &evidence_series(report.clone()),
            &tampered,
            &BTreeMap::new(),
        ),
        Err(ReleaseMatrixError::InvalidRomClassAuthority { .. })
    ));

    let mut wrong_semantic_report = rom_class_authority(&report);
    wrong_semantic_report.semantic_report_sha256 = "95".repeat(32);
    wrong_semantic_report.authority_sha256 = wrong_semantic_report.recompute_authority_sha256();
    let wrong_semantic_report =
        BTreeMap::from([(report.scenario.clone(), wrong_semantic_report)]);
    assert!(matches!(
        verify_release_matrix_with_authorities(
            &manifest,
            &evidence_series(report.clone()),
            &wrong_semantic_report,
            &BTreeMap::new(),
        ),
        Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            field: "semantic_report_sha256",
            ..
        })
    ));

    let mut reordered_runs = rom_class_authority(&report);
    reordered_runs.run_event_sha256s.swap(0, 1);
    reordered_runs.authority_sha256 = reordered_runs.recompute_authority_sha256();
    let reordered_runs = BTreeMap::from([(report.scenario.clone(), reordered_runs)]);
    assert!(matches!(
        verify_release_matrix_with_authorities(
            &manifest,
            &evidence_series(report),
            &reordered_runs,
            &BTreeMap::new(),
        ),
        Err(ReleaseMatrixError::RomClassAuthorityMismatch {
            field: "run_event_sha256s",
            ..
        })
    ));
}

#[test]
fn private_series_authorities_reject_unused_and_duplicate_records() {
    let report = with_rom(
        closed_report(
            "authority-homebrew",
            b"placeholder",
            0xd5,
            "save.sram-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        ),
        b'E',
        ReleaseRomClass::PublicHomebrew,
    );
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("authority-homebrew", &report)],
    };
    let authority = rom_class_authority(&report);
    let mut duplicate = BTreeMap::new();
    insert_private_series_authority(&mut duplicate, report.scenario.clone(), authority.clone())
        .unwrap();
    assert!(matches!(
        insert_private_series_authority(
            &mut duplicate,
            report.scenario.clone(),
            authority.clone(),
        ),
        Err(ReleaseMatrixError::DuplicatePrivateSeriesAuthority { .. })
    ));

    let unused = BTreeMap::from([("unrelated-scenario".to_owned(), authority)]);
    assert!(matches!(
        validate_private_series_authority_usage(&manifest, &unused),
        Err(ReleaseMatrixError::UnusedPrivateSeriesAuthority { .. })
    ));
}

#[test]
fn flat_evidence_auto_routes_by_report_scenario_not_manifest_id_or_input_order() {
    let (manifest, mut evidence) = fixture();
    assert_ne!(
        manifest.scenarios[0].id,
        manifest.scenarios[0].report_scenario
    );
    evidence.rotate_left(7);
    let ReleaseMatrixVerification::Incomplete(incomplete) =
        verify_release_matrix(&manifest, &evidence).unwrap()
    else {
        panic!("fixture remains intentionally incomplete")
    };
    assert_eq!(incomplete.verified_scenarios, 2);
    assert_eq!(incomplete.verified_reports, 20);
}

#[test]
fn profile_identity_and_v4_relabels_are_rejected() {
    for (schema, digest, expected_schema_error) in [
        (
            "fn64.certification-profile.full-parity.v0",
            crate::FULL_PARITY_V1_DEFINITION_SHA256,
            true,
        ),
        (crate::FULL_PARITY_V1_SCHEMA, "00", false),
    ] {
        let (mut manifest, evidence) = fixture();
        manifest.profile.schema = schema.to_owned();
        manifest.profile.definition_sha256 = digest.to_owned();
        let error = verify_release_matrix(&manifest, &evidence).unwrap_err();
        match (error, expected_schema_error) {
            (
                ReleaseMatrixError::InvalidCertificationProfile(
                    crate::CertificationProfileError::UnsupportedSchema(_),
                ),
                true,
            )
            | (
                ReleaseMatrixError::InvalidCertificationProfile(
                    crate::CertificationProfileError::DefinitionDigestMismatch { .. },
                ),
                false,
            ) => {}
            (other, _) => panic!("unexpected profile error: {other:?}"),
        }
    }

    let (mut manifest, evidence) = fixture();
    manifest.schema = "fn64.release-matrix.v4".to_owned();
    assert!(matches!(
        verify_release_matrix(&manifest, &evidence),
        Err(ReleaseMatrixError::UnsupportedSchema(schema))
            if schema == "fn64.release-matrix.v4"
    ));

    let legacy = serde_json::json!({
        "schema": RELEASE_MATRIX_SCHEMA,
        "required": {
            "platforms": ["macos_arm64"],
            "controllers": ["standard_controller"],
            "saves": ["eeprom_4_kbit"],
            "renderers": ["reference_lle_accuracy"],
            "programs": ["typed_observed_function"]
        },
        "scenarios": []
    });
    assert!(serde_json::from_value::<ReleaseMatrixManifest>(legacy).is_err());
}

#[test]
fn scenario_and_manifest_digests_use_the_v5_evidence_only_wire() {
    let (manifest, evidence) = fixture();
    let declaration = &manifest.scenarios[0];
    let baseline = declaration.recompute_declaration_sha256();

    let mut legacy_wire = Vec::new();
    legacy_wire.extend_from_slice(b"fn64.release-matrix.scenario.v4\0");
    push_bytes(&mut legacy_wire, declaration.id.as_bytes());
    push_bytes(&mut legacy_wire, declaration.report_scenario.as_bytes());
    push_bytes(&mut legacy_wire, declaration.input_sha256.as_bytes());
    push_bytes(&mut legacy_wire, declaration.report_sha256.as_bytes());
    assert_ne!(baseline, hex(&Sha256::digest(legacy_wire)));

    for field in ["id", "scenario", "input", "report"] {
        let mut changed = declaration.clone();
        match field {
            "id" => changed.id.push('x'),
            "scenario" => changed.report_scenario.push('x'),
            "input" => changed.input_sha256 = "11".repeat(32),
            "report" => changed.report_sha256 = "22".repeat(32),
            _ => unreachable!(),
        }
        assert_ne!(changed.recompute_declaration_sha256(), baseline, "{field}");
    }

    let mut relabeled = manifest.clone();
    relabeled.scenarios[0].report_scenario.push_str("-changed");
    assert!(matches!(
        verify_release_matrix(&relabeled, &evidence),
        Err(ReleaseMatrixError::DeclarationDigestMismatch { .. })
    ));

    let baseline_manifest = manifest.recompute_manifest_sha256();
    let mut changed_profile = manifest;
    changed_profile.profile.definition_sha256 = "33".repeat(32);
    assert_ne!(
        changed_profile.recompute_manifest_sha256(),
        baseline_manifest
    );
}

#[test]
fn exact_ten_run_series_is_enforced_table_driven() {
    for requested in [9usize, 11] {
        let (manifest, evidence) = fixture();
        let mut first: Vec<_> = evidence
            .iter()
            .filter(|(report, _)| report.scenario == "game-a-reference")
            .cloned()
            .collect();
        if requested == 9 {
            first.pop();
        } else {
            let extra = first[0].clone();
            let mut extra = extra;
            let crate::UnsupportedJournalCompletion::V3RunBound {
                run_event_sha256, ..
            } = &mut extra.1.completion
            else {
                unreachable!()
            };
            *run_event_sha256 = "44".repeat(32);
            first.push(extra);
        }
        let mut changed: Vec<_> = evidence
            .into_iter()
            .filter(|(report, _)| report.scenario != "game-a-reference")
            .collect();
        changed.extend(first);
        assert!(matches!(
            verify_release_matrix(&manifest, &changed),
            Err(ReleaseMatrixError::WrongReportCount {
                expected: 10,
                actual,
                ..
            }) if actual == requested
        ));
    }
}

#[test]
fn missing_and_unexpected_scenario_evidence_are_distinct() {
    let (manifest, evidence) = fixture();
    let missing: Vec<_> = evidence
        .iter()
        .filter(|(report, _)| report.scenario != "game-a-reference")
        .cloned()
        .collect();
    assert!(matches!(
        verify_release_matrix(&manifest, &missing),
        Err(ReleaseMatrixError::MissingEvidence { id })
            if id == "reference-evidence"
    ));

    let unexpected = closed_report(
        "undeclared-report",
        b"private-c",
        0xc3,
        "save.eeprom-4k-operation",
        CLEAN_RT64_IDENTITY,
        Some(ProgramFeature::TypedObservedFunction),
    );
    let mut extra = evidence;
    extra.extend(evidence_series(unexpected));
    assert!(matches!(
        verify_release_matrix(&manifest, &extra),
        Err(ReleaseMatrixError::UnexpectedReportScenario { scenario })
            if scenario == "undeclared-report"
    ));
}

#[test]
fn replayed_run_event_identity_across_scenarios_is_rejected() {
    let (manifest, mut evidence) = fixture();
    let replay = evidence
        .iter()
        .find_map(|(report, journal)| {
            (report.scenario == "game-a-reference").then(|| match &journal.completion {
                crate::UnsupportedJournalCompletion::V3RunBound {
                    run_event_sha256, ..
                } => run_event_sha256.clone(),
                _ => unreachable!(),
            })
        })
        .unwrap();
    let (_, journal) = evidence
        .iter_mut()
        .find(|(report, _)| report.scenario == "game-b-rt64")
        .unwrap();
    let crate::UnsupportedJournalCompletion::V3RunBound {
        run_event_sha256, ..
    } = &mut journal.completion
    else {
        unreachable!()
    };
    *run_event_sha256 = replay;
    assert!(matches!(
        verify_release_matrix(&manifest, &evidence),
        Err(ReleaseMatrixError::DuplicateRunEventIdentity { .. })
    ));
}

#[test]
fn report_and_input_digests_are_bound_by_each_v5_declaration() {
    for field in ["input", "report"] {
        let (mut manifest, evidence) = fixture();
        if field == "input" {
            manifest.scenarios[0].input_sha256 = "55".repeat(32);
        } else {
            manifest.scenarios[0].report_sha256 = "66".repeat(32);
        }
        manifest.scenarios[0].declaration_sha256 =
            manifest.scenarios[0].recompute_declaration_sha256();
        let error = verify_release_matrix(&manifest, &evidence).unwrap_err();
        assert!(
            matches!(
                (&error, field),
                (ReleaseMatrixError::InputDigestMismatch { .. }, "input")
                    | (ReleaseMatrixError::ReportDigestMismatch { .. }, "report")
            ),
            "unexpected {field} result: {error:?}"
        );
    }
}

#[test]
fn report_without_entered_program_evidence_is_rejected() {
    let (mut manifest, mut evidence) = fixture();
    let original = evidence
        .iter()
        .find(|(report, _)| report.scenario == "game-a-reference")
        .unwrap()
        .0
        .clone();
    let replacement = ReleaseGateReport::new_with_test_environment(
        original.scenario.clone(),
        b"private-a",
        original.digest,
        original.observations,
        original.environment,
        original.closure,
    )
    .unwrap();
    replace_report(&mut manifest, &mut evidence, replacement);
    assert!(matches!(
        verify_release_matrix(&manifest, &evidence),
        Err(ReleaseMatrixError::NoProgramEvidence { .. })
    ));
}

#[test]
fn rt64_identity_and_observation_source_are_authoritative() {
    const DIRTY_RT64_IDENTITY: &str = concat!(
        "adapter=fn64-render-rt64/rt64;adapter_sha256=",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ";source=git:",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ";provenance=git-dirty;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
    );
    let (_, evidence) = fixture();
    let original = evidence
        .iter()
        .find(|(report, _)| report.scenario == "game-b-rt64")
        .unwrap()
        .0
        .clone();
    let mut environment = original.environment.clone();
    let ReleaseRendererEvidence::Rt64 {
        backend_identity, ..
    } = &mut environment.renderer
    else {
        unreachable!()
    };
    *backend_identity = DIRTY_RT64_IDENTITY.to_owned();
    let mut observations = original.observations.clone();
    let FramebufferObservationSource::PostViSwapchain {
        backend_identity, ..
    } = &mut observations.framebuffer.source
    else {
        unreachable!()
    };
    *backend_identity = DIRTY_RT64_IDENTITY.to_owned();
    assert!(matches!(
        ReleaseGateReport::new_with_test_environment_and_destinations(
            original.scenario,
            b"private-b",
            original.digest,
            observations,
            environment,
            original.execution_destinations,
            original.closure,
        ),
        Err(crate::GateError::RendererObservationMismatch(_))
    ));

    let (manifest, mut evidence) = fixture();
    let report = evidence
        .iter_mut()
        .find(|(report, _)| report.scenario == "game-b-rt64")
        .unwrap();
    report.0.environment.renderer = ReleaseRendererEvidence::Reference {
        execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
        tv_type: crate::ReleaseTvStandard::Ntsc,
    };
    assert!(matches!(
        verify_release_matrix(&manifest, &evidence),
        Err(ReleaseMatrixError::InvalidUnassignedReport {
            source: crate::GateError::RendererObservationMismatch(_),
            ..
        })
    ));
}

#[test]
fn save_and_controller_assignments_require_positive_operation_paths() {
    for (scenario_name, input, path) in [
        (
            "game-a-reference",
            b"private-a".as_slice(),
            "save.eeprom-4k-operation",
        ),
        (
            "game-b-rt64",
            b"private-b".as_slice(),
            "controller.rumble-operation",
        ),
    ] {
        let (mut manifest, mut evidence) = fixture();
        let original = evidence
            .iter()
            .find(|(report, _)| report.scenario == scenario_name)
            .unwrap()
            .0
            .clone();
        let closure: Vec<_> = original
            .closure
            .iter()
            .filter(|entry| entry.name != path)
            .cloned()
            .collect();
        let replacement = ReleaseGateReport::new_with_test_environment_and_destinations(
            original.scenario,
            input,
            original.digest,
            original.observations,
            original.environment,
            original.execution_destinations,
            closure,
        )
        .unwrap();
        replace_report(&mut manifest, &mut evidence, replacement);
        assert!(matches!(
            verify_release_matrix(&manifest, &evidence),
            Err(ReleaseMatrixError::MissingFeatureObservation {
                path: missing,
                ..
            }) if missing == path
        ));
    }
}

#[test]
fn program_renderer_save_and_controller_coverage_is_derived_from_reports() {
    let (_, evidence, incomplete) = incomplete_fixture();
    let reference = evidence
        .iter()
        .find(|(report, _)| report.scenario == "game-a-reference")
        .unwrap();
    let rt64 = evidence
        .iter()
        .find(|(report, _)| report.scenario == "game-b-rt64")
        .unwrap();

    assert_eq!(
        derive_scenario_coverage("reference-evidence", &reference.0).unwrap(),
        ReleaseMatrixCoverage {
            rom_classes: Vec::new(),
            tv_regions: Vec::new(),
            platforms: vec![ReleasePlatform::MacosArm64],
            controllers: vec![ControllerFeature::StandardController],
            saves: vec![SaveFeature::Eeprom4Kbit],
            renderers: vec![RendererFeature::ReferenceLleAccuracy],
            programs: vec![ProgramFeature::TypedObservedFunction],
            microcodes: Vec::new(),
            rsp_rdp_mechanisms: Vec::new(),
        }
    );
    assert_eq!(
        derive_scenario_coverage("rt64-evidence", &rt64.0).unwrap(),
        ReleaseMatrixCoverage {
            rom_classes: Vec::new(),
            tv_regions: Vec::new(),
            platforms: vec![ReleasePlatform::LinuxX86_64],
            controllers: vec![
                ControllerFeature::StandardController,
                ControllerFeature::RumblePak,
            ],
            saves: vec![SaveFeature::Sram32Kib],
            renderers: vec![
                RendererFeature::Rt64LleAccuracy,
                RendererFeature::Rt64PostViCapture,
            ],
            programs: vec![ProgramFeature::TypedObservedFunction],
            microcodes: Vec::new(),
            rsp_rdp_mechanisms: Vec::new(),
        }
    );

    let assigned: BTreeSet<_> = incomplete
        .satisfied
        .iter()
        .map(|assignment| {
            (
                assignment.requirement.class(),
                assignment.requirement.id().to_owned(),
            )
        })
        .collect();
    for key in [
        (
            CertificationRequirementClass::ProgramRendererLane,
            "typed_observed_function/reference_lle_accuracy",
        ),
        (
            CertificationRequirementClass::ProgramRendererLane,
            "typed_observed_function/rt64_lle_accuracy",
        ),
        (CertificationRequirementClass::Save, "eeprom_4_kbit"),
        (CertificationRequirementClass::Save, "sram_32_kib"),
        (
            CertificationRequirementClass::Controller,
            "standard_controller",
        ),
        (CertificationRequirementClass::Controller, "rumble_pak"),
        (
            CertificationRequirementClass::PlatformApiTarget,
            "linux-vulkan",
        ),
    ] {
        assert!(assigned.contains(&(key.0, key.1.to_owned())), "{key:?}");
    }
}

#[test]
fn exact_macos_metal_and_linux_vulkan_evidence_receive_platform_credit() {
    assert_eq!(
        clean_rt64_identity_for(ReleaseGraphicsApi::Vulkan),
        CLEAN_RT64_IDENTITY,
    );
    for (scenario_name, platform, graphics_api, expected) in [
        (
            "exact-macos-rt64",
            ReleaseHostPlatform::MacosArm64,
            ReleaseGraphicsApi::Metal,
            "macos-metal",
        ),
        (
            "exact-linux-rt64",
            ReleaseHostPlatform::LinuxX86_64,
            ReleaseGraphicsApi::Vulkan,
            "linux-vulkan",
        ),
    ] {
        let report = rt64_report_for_platform_api(
            scenario_name,
            scenario_name.as_bytes(),
            platform,
            graphics_api,
        );
        let incomplete = incomplete_for_report(scenario_name, report);
        assert_eq!(
            assigned_requirement_ids(
                &incomplete,
                CertificationRequirementClass::PlatformApiTarget,
            ),
            BTreeSet::from([expected.to_owned()]),
        );
    }
}

#[test]
fn opaque_platform_case_authority_binds_exact_matrix_series() {
    let seed = 0x51;
    let adapter_sha256 = hex(&Sha256::digest([seed, 0]));
    let identity = pinned_platform_identity(ReleaseGraphicsApi::Metal, &adapter_sha256);
    let report = closed_report_with_rt64_environment(
        "bound-macos-rt64-platform-case",
        b"bound-macos-rt64-platform-case",
        0xc7,
        "save.sram-operation",
        &identity,
        Some(ProgramFeature::TypedObservedFunction),
        ReleaseHostPlatform::MacosArm64,
        ReleaseGraphicsApi::Metal,
    );
    let evidence = evidence_series(report.clone());
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("bound-macos-rt64-platform-case", &report)],
    };
    let series = platform_case_fixture(
        &report,
        &evidence,
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::ResolutionDownsample,
        seed,
    );
    let ReleaseMatrixVerification::Incomplete(incomplete) =
        verify_release_matrix_with_platform_series(&manifest, &evidence, &[&series]).unwrap()
    else {
        panic!("one synthetic report remains incomplete")
    };
    let case_id = "macos-metal/resolution-downsample";
    let assignment = incomplete
        .satisfied
        .iter()
        .find(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::Rt64TargetCase
                && assignment.requirement.id() == case_id
        })
        .expect("opaque authority earns only its exact case row");
    assert_eq!(assignment.evidence_sha256s.len(), 1);
    let platform_assignment = incomplete
        .satisfied
        .iter()
        .find(|assignment| {
            assignment.requirement.class() == CertificationRequirementClass::PlatformApiTarget
                && assignment.requirement.id() == "macos-metal"
        })
        .expect("the validated v30 report earns its exact platform/API row");
    assert_eq!(
        platform_assignment.evidence_sha256s,
        [manifest.scenarios[0].declaration_sha256.clone()]
    );
    assert_eq!(incomplete.platform_case_authorities.len(), 1);

    let mut detached_retained = incomplete.clone();
    detached_retained.platform_case_authorities.clear();
    detached_retained.assessment_sha256 = incomplete_matrix_sha256(&detached_retained);
    assert!(matches!(
        detached_retained.verify_integrity(),
        Err(ReleaseMatrixError::PlatformAuthorityAssignmentMismatch { .. })
    ));

    assert!(matches!(
        verify_release_matrix_with_platform_series(&manifest, &evidence, &[&series, &series],),
        Err(ReleaseMatrixError::DuplicatePlatformSeriesAuthority { .. })
    ));
}

#[test]
fn platform_case_authority_rejects_detached_report_and_run_events() {
    let seed = 0x52;
    let adapter_sha256 = hex(&Sha256::digest([seed, 0]));
    let identity = pinned_platform_identity(ReleaseGraphicsApi::Metal, &adapter_sha256);
    let report = closed_report_with_rt64_environment(
        "rt64-platform-binding-original",
        b"rt64-platform-binding-original",
        0xc8,
        "save.sram-operation",
        &identity,
        Some(ProgramFeature::TypedObservedFunction),
        ReleaseHostPlatform::MacosArm64,
        ReleaseGraphicsApi::Metal,
    );
    let evidence = evidence_series(report.clone());
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("rt64-platform-binding-original", &report)],
    };
    let verified =
        verify_release_evidence_series(&evidence, RELEASE_MATRIX_REPORT_COUNT).unwrap();
    let detached = VerifiedRt64PlatformCaseSeries::fixture_for_test(
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::FramebufferEnhancement,
        (ReleaseHostPlatform::MacosArm64, None),
        (
            "different-report-scenario",
            report.report_sha256.clone(),
            verified.run_event_sha256s.clone(),
        ),
        seed,
    )
    .unwrap();
    assert!(matches!(
        verify_release_matrix_with_platform_series(&manifest, &evidence, &[&detached]),
        Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority { .. })
    ));

    let wrong_report_sha = VerifiedRt64PlatformCaseSeries::fixture_for_test(
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::FramebufferEnhancement,
        (ReleaseHostPlatform::MacosArm64, None),
        (
            &report.scenario,
            hex(&Sha256::digest(b"different-semantic-report")),
            verified.run_event_sha256s.clone(),
        ),
        seed,
    )
    .unwrap();
    assert!(matches!(
        verify_release_matrix_with_platform_series(&manifest, &evidence, &[&wrong_report_sha],),
        Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority { .. })
    ));

    let mut reordered_events = verified.run_event_sha256s;
    reordered_events.rotate_left(1);
    let reordered = VerifiedRt64PlatformCaseSeries::fixture_for_test(
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::FramebufferEnhancement,
        (ReleaseHostPlatform::MacosArm64, None),
        (
            &report.scenario,
            report.report_sha256.clone(),
            reordered_events,
        ),
        seed,
    )
    .unwrap();
    assert!(matches!(
        verify_release_matrix_with_platform_series(&manifest, &evidence, &[&reordered]),
        Err(ReleaseMatrixError::UnusedPlatformSeriesAuthority { .. })
    ));
}

#[test]
fn exact_windows_build_and_observed_api_receive_only_their_target_credit() {
    for (scenario_name, build, graphics_api, expected) in [
        (
            "exact-windows10-d3d12-rt64",
            21_999,
            ReleaseGraphicsApi::D3d12,
            "windows10-d3d12",
        ),
        (
            "exact-windows10-vulkan-rt64",
            21_999,
            ReleaseGraphicsApi::Vulkan,
            "windows10-vulkan",
        ),
        (
            "exact-windows11-d3d12-rt64",
            22_000,
            ReleaseGraphicsApi::D3d12,
            "windows11-d3d12",
        ),
        (
            "exact-windows11-vulkan-rt64",
            22_000,
            ReleaseGraphicsApi::Vulkan,
            "windows11-vulkan",
        ),
    ] {
        let mut report = rt64_report_for_platform_api(
            scenario_name,
            scenario_name.as_bytes(),
            ReleaseHostPlatform::WindowsX86_64,
            graphics_api,
        );
        report.environment.windows_version = Some(
            crate::ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, build, 123)
                .unwrap(),
        );
        report.report_sha256 = hex(&Sha256::digest(
            crate::release_gate::encode_report_evidence(&report).unwrap(),
        ));
        let incomplete = incomplete_for_report(scenario_name, report);
        assert_eq!(
            assigned_requirement_ids(
                &incomplete,
                CertificationRequirementClass::PlatformApiTarget,
            ),
            BTreeSet::from([expected.to_owned()]),
        );
    }
}

#[test]
fn windows_family_relabel_cannot_manufacture_platform_credit() {
    let mut report = rt64_report_for_platform_api(
        "relabeled-windows11-d3d12-rt64",
        b"relabeled-windows11-d3d12-rt64",
        ReleaseHostPlatform::WindowsX86_64,
        ReleaseGraphicsApi::D3d12,
    );
    let mut version =
        crate::ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 21_999, 123)
            .unwrap();
    version.family = ReleaseWindowsFamily::Windows11;
    report.environment.windows_version = Some(version);
    report.report_sha256 = hex(&Sha256::digest(
        crate::release_gate::encode_report_evidence(&report).unwrap(),
    ));
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("relabeled-windows", &report)],
    };
    assert!(matches!(
        verify_release_matrix(&manifest, &evidence_series(report)),
        Err(ReleaseMatrixError::InvalidUnassignedReport {
            source: crate::GateError::InvalidWindowsVersionEvidence(_),
            ..
        })
    ));
}

#[test]
fn reference_platform_and_scenario_label_do_not_substitute_for_api_evidence() {
    let report = closed_report(
        "macos-metal",
        b"reference-platform-only",
        0xd4,
        "save.eeprom-4k-operation",
        CLEAN_RT64_IDENTITY,
        Some(ProgramFeature::TypedObservedFunction),
    );
    assert_eq!(report.environment.platform, ReleaseHostPlatform::MacosArm64);
    assert!(matches!(
        report.environment.renderer,
        ReleaseRendererEvidence::Reference { .. }
    ));
    let incomplete = incomplete_for_report("macos-metal", report);
    assert!(assigned_requirement_ids(
        &incomplete,
        CertificationRequirementClass::PlatformApiTarget,
    )
    .is_empty());
}

#[test]
fn graphics_api_changes_bind_environment_report_and_matrix_digests() {
    let d3d12 = rt64_report_for_platform_api(
        "windows-api-rt64",
        b"same-private-input",
        ReleaseHostPlatform::WindowsX86_64,
        ReleaseGraphicsApi::D3d12,
    );
    let vulkan = rt64_report_for_platform_api(
        "windows-api-rt64",
        b"same-private-input",
        ReleaseHostPlatform::WindowsX86_64,
        ReleaseGraphicsApi::Vulkan,
    );

    let environment_sha256 = |environment: &ReleaseEnvironmentEvidence| {
        let mut wire = Vec::new();
        push_environment(&mut wire, environment);
        hex(&Sha256::digest(wire))
    };
    let mut api_tag_only = d3d12.environment.clone();
    let ReleaseRendererEvidence::Rt64 { graphics_api, .. } = &mut api_tag_only.renderer else {
        unreachable!()
    };
    *graphics_api = ReleaseGraphicsApi::Vulkan;
    assert_ne!(
        environment_sha256(&d3d12.environment),
        environment_sha256(&api_tag_only),
        "the typed API tag must bind the environment wire independently of the identity string",
    );
    assert_ne!(
        environment_sha256(&d3d12.environment),
        environment_sha256(&vulkan.environment),
    );
    assert_ne!(d3d12.report_sha256, vulkan.report_sha256);

    let d3d12_declaration = scenario("windows-api", &d3d12);
    let vulkan_declaration = scenario("windows-api", &vulkan);
    assert_ne!(
        d3d12_declaration.declaration_sha256,
        vulkan_declaration.declaration_sha256,
    );
    assert_ne!(
        incomplete_for_report("windows-api", d3d12).assessment_sha256,
        incomplete_for_report("windows-api", vulkan).assessment_sha256,
    );
}

#[test]
fn stale_verified_v16_and_incomplete_v5_schemas_are_rejected() {
    let (_, _, incomplete) = incomplete_fixture();
    let mut stale_incomplete = incomplete;
    stale_incomplete.schema = "fn64.release-matrix-incomplete.v5".to_owned();
    assert!(matches!(
        stale_incomplete.verify_integrity(),
        Err(ReleaseMatrixError::UnsupportedIncompleteSchema(schema))
            if schema == "fn64.release-matrix-incomplete.v5"
    ));

    let stale_verified = VerifiedReleaseMatrix {
        schema: "fn64.verified-release-matrix.v16".to_owned(),
        manifest_sha256: "00".repeat(32),
        profile: CertificationProfileIdentity::full_parity_v1(),
        total_reports: 0,
        scenarios: Vec::new(),
        platform_case_authorities: Vec::new(),
        assignments: Vec::new(),
        verification_sha256: "11".repeat(32),
    };
    assert!(matches!(
        stale_verified.verify_integrity(),
        Err(ReleaseMatrixError::UnsupportedVerifiedSchema(schema))
            if schema == "fn64.verified-release-matrix.v16"
    ));
}
