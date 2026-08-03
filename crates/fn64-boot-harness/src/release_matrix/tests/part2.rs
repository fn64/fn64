use super::*;

#[test]
fn microcode_credit_requires_project_catalog_and_rsp_rdp_uses_report_events() {
    let public_families = [
        ReleaseMicrocodeFamily::Fast3d,
        ReleaseMicrocodeFamily::F3dex,
        ReleaseMicrocodeFamily::F3dlx,
        ReleaseMicrocodeFamily::F3dlxRej,
        ReleaseMicrocodeFamily::F3dex2,
        ReleaseMicrocodeFamily::F3dex2NoN,
        ReleaseMicrocodeFamily::F3dex2Rej,
        ReleaseMicrocodeFamily::F3dlx2Rej,
        ReleaseMicrocodeFamily::S2dex,
        ReleaseMicrocodeFamily::S2dex2,
        ReleaseMicrocodeFamily::L3dex,
        ReleaseMicrocodeFamily::L3dex2,
    ];
    let certified_digest_families = public_families
        .iter()
        .copied()
        .enumerate()
        .map(|(index, family)| ([u8::try_from(index + 1).unwrap(); 32], family))
        .collect::<Vec<_>>();
    let certified_catalog = certified_digest_families
        .iter()
        .map(|(text_sha256, family)| CertifiedMicrocodeIdentity {
            text_sha256: *text_sha256,
            family: *family,
        })
        .collect::<Vec<_>>();
    let mut ordered = certified_digest_families
        .iter()
        .enumerate()
        .map(
            |(index, (text_sha256, family))| RspRdpObservationEventEvidence {
                guest_cycle: 50,
                observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                    task_address: 0x1000 + index as u32 * 0x40,
                    imem_generation: index as u64 + 1,
                    text_sha256: hex(text_sha256),
                    data_address: 0x4000 + index as u32 * 0x80,
                    data_bytes: 0x80,
                    data_sha256: hex(text_sha256),
                    family: Some(*family),
                },
            },
        )
        .collect::<Vec<_>>();
    for family in [
        ReleaseMicrocodeFamily::F3dzex2,
        ReleaseMicrocodeFamily::Other { id: 7 },
    ] {
        ordered.push(RspRdpObservationEventEvidence {
            guest_cycle: 51,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0x2000,
                imem_generation: 20,
                text_sha256: "ee".repeat(32),
                data_address: 0x6000,
                data_bytes: 0x80,
                data_sha256: "ed".repeat(32),
                family: Some(family),
            },
        });
    }
    ordered.push(RspRdpObservationEventEvidence {
        guest_cycle: 52,
        observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
            task_address: 0x2040,
            imem_generation: 21,
            text_sha256: "ef".repeat(32),
            data_address: 0x6080,
            data_bytes: 0x80,
            data_sha256: "ec".repeat(32),
            family: None,
        },
    });
    ordered.extend([
        RspRdpObservationEventEvidence {
            guest_cycle: 53,
            observation: RspRdpObservationKindEvidence::DramDpcCommitted {
                start: 0x100,
                end: 0x108,
                command_sha256: "f1".repeat(32),
            },
        },
        RspRdpObservationEventEvidence {
            guest_cycle: 54,
            observation: RspRdpObservationKindEvidence::XbusDpcCommitted {
                start: 0,
                end: 8,
                command_sha256: "f2".repeat(32),
            },
        },
        RspRdpObservationEventEvidence {
            guest_cycle: 55,
            observation: RspRdpObservationKindEvidence::ImemReplacementCommitted {
                task_address: 0x3000,
                imem_generation: 22,
                text_sha256: "f3".repeat(32),
            },
        },
    ]);
    let report = with_rsp_rdp_observations(
        closed_report(
            "microcode-reference",
            b"private-microcode",
            0xc3,
            "save.eeprom-4k-operation",
            CLEAN_RT64_IDENTITY,
            Some(ProgramFeature::TypedObservedFunction),
        ),
        ordered,
    );
    let certified_coverage = derive_scenario_coverage_with_catalog(
        "microcode-evidence",
        &report,
        &certified_catalog,
    )
    .unwrap();
    assert_eq!(certified_coverage.microcodes.len(), 12);
    assert_eq!(
        certified_coverage.rsp_rdp_mechanisms,
        vec![
            RspRdpMechanismFeature::DramDpc,
            RspRdpMechanismFeature::XbusDpc,
            RspRdpMechanismFeature::ImemReplacement,
        ]
    );

    let production_coverage = derive_scenario_coverage("microcode-evidence", &report).unwrap();
    assert!(production_coverage.microcodes.is_empty());
    assert_eq!(
        production_coverage.rsp_rdp_mechanisms,
        certified_coverage.rsp_rdp_mechanisms
    );

    let mut mislabeled = report.clone();
    let RspRdpObservationKindEvidence::MicrocodeRecognition { family, .. } =
        &mut mislabeled.rsp_rdp.ordered[0].observation
    else {
        panic!("first fixture event must be microcode recognition");
    };
    *family = Some(ReleaseMicrocodeFamily::F3dex);
    assert!(matches!(
        derive_scenario_coverage_with_catalog(
            "microcode-evidence",
            &mislabeled,
            &certified_catalog
        ),
        Err(ReleaseMatrixError::CertifiedMicrocodeFamilyMismatch {
            certified: ReleaseMicrocodeFamily::Fast3d,
            observed: ReleaseMicrocodeFamily::F3dex,
            ..
        })
    ));

    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario("microcode-evidence", &report)],
    };
    let ReleaseMatrixVerification::Incomplete(incomplete) =
        verify_release_matrix(&manifest, &evidence_series(report)).unwrap()
    else {
        panic!("platform and full-ROM requirements remain intentionally absent");
    };
    for (class, id) in [
        (CertificationRequirementClass::RspRdpMechanism, "dram-dpc"),
        (
            CertificationRequirementClass::RspRdpMechanism,
            "imem-replacement",
        ),
    ] {
        assert!(incomplete.satisfied.iter().any(|assignment| {
            assignment.requirement.class() == class && assignment.requirement.id() == id
        }));
    }
    assert!(!incomplete.satisfied.iter().any(|assignment| {
        assignment.requirement.class() == CertificationRequirementClass::PublicMicrocode
    }));
}

#[test]
fn incomplete_integrity_rejects_cross_class_duplicates_partition_and_hash_tampering() {
    let (_, _, baseline) = incomplete_fixture();

    let mut instrumentation_drift = baseline.clone();
    instrumentation_drift.unsupported_instrumentation.schema =
        "fn64.unsupported-instrumentation.future".to_owned();
    instrumentation_drift.assessment_sha256 = incomplete_matrix_sha256(&instrumentation_drift);
    assert!(matches!(
        instrumentation_drift.verify_integrity(),
        Err(ReleaseMatrixError::InvalidUnsupportedInstrumentation(_))
    ));

    let mut cross_class = baseline.clone();
    cross_class.missing[0] =
        forged_ref(CertificationRequirementClass::Save, "standard_controller");
    assert!(matches!(
        cross_class.verify_integrity(),
        Err(ReleaseMatrixError::InvalidCertificationProfile(
            crate::CertificationProfileError::UnknownRequirement { .. }
        ))
    ));

    let mut duplicate = baseline.clone();
    duplicate.satisfied.push(duplicate.satisfied[0].clone());
    assert!(matches!(
        duplicate.verify_integrity(),
        Err(ReleaseMatrixError::DuplicateRequirementAssignment { .. })
    ));

    let mut overlap = baseline.clone();
    overlap
        .missing
        .push(overlap.satisfied[0].requirement.clone());
    assert!(matches!(
        overlap.verify_integrity(),
        Err(ReleaseMatrixError::DuplicateRequirementAssignment { .. })
    ));

    let mut missing_partition = baseline.clone();
    missing_partition.missing.pop();
    assert!(matches!(
        missing_partition.verify_integrity(),
        Err(ReleaseMatrixError::InvalidRequirementPartition)
    ));

    let mut duplicate_evidence = baseline.clone();
    let digest = duplicate_evidence.satisfied[0].evidence_sha256s[0].clone();
    duplicate_evidence.satisfied[0]
        .evidence_sha256s
        .push(digest);
    assert!(matches!(
        duplicate_evidence.verify_integrity(),
        Err(ReleaseMatrixError::DuplicateRequirementEvidence { .. })
    ));

    let mut malformed_evidence = baseline.clone();
    malformed_evidence.satisfied[0].evidence_sha256s[0] = "not-a-sha".to_owned();
    assert!(matches!(
        malformed_evidence.verify_integrity(),
        Err(ReleaseMatrixError::InvalidSha256 {
            field: "evidence_sha256s",
            ..
        })
    ));

    let mut empty_evidence = baseline.clone();
    empty_evidence.satisfied[0].evidence_sha256s.clear();
    assert!(matches!(
        empty_evidence.verify_integrity(),
        Err(ReleaseMatrixError::EmptyRequirementEvidence { .. })
    ));

    let mut semantic_hash_change = baseline.clone();
    semantic_hash_change.satisfied[0].evidence_sha256s[0] = "77".repeat(32);
    assert!(matches!(
        semantic_hash_change.verify_integrity(),
        Err(ReleaseMatrixError::IncompleteIntegrityMismatch { .. })
    ));

    let mut assessment_hash = baseline;
    assessment_hash.assessment_sha256 = "88".repeat(32);
    assert!(matches!(
        assessment_hash.verify_integrity(),
        Err(ReleaseMatrixError::IncompleteIntegrityMismatch { .. })
    ));
}
