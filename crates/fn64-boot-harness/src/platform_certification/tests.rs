use super::*;

fn rt64_renderer(
    target: Rt64PlatformTarget,
    adapter_source_sha256: &str,
) -> ReleaseRendererEvidence {
    ReleaseRendererEvidence::Rt64 {
        execution_policy: crate::ReleaseGraphicsExecutionPolicy::LleAccuracy,
        tv_type: crate::ReleaseTvStandard::Ntsc,
        graphics_api: target.graphics_api(),
        backend_identity: format!(
            "adapter=fn64-render-rt64/rt64;adapter_sha256={adapter_source_sha256};source={PINNED_RT64_SOURCE_ID};provenance=git-clean;overlay=none;post_vi_api={}",
            target.capture_api()
        ),
        source_authoritative: true,
        settings_sha256: "11".repeat(32),
        replacement_packs_active: false,
    }
}

fn windows(build: u32) -> ReleaseWindowsVersionEvidence {
    ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, build, 123).unwrap()
}

fn bound_events(seed: u8) -> Vec<String> {
    (0..crate::RELEASE_MATRIX_REPORT_COUNT)
        .map(|ordinal| hex(&Sha256::digest([seed, ordinal as u8, 9])))
        .collect()
}

fn bound_report(seed: u8) -> String {
    hex(&Sha256::digest([seed, 8]))
}

#[test]
fn windows_family_is_derived_from_build_and_relabel_fails() {
    let ten = windows(21_999);
    let eleven = windows(22_000);
    assert_eq!(ten.family, ReleaseWindowsFamily::Windows10);
    assert_eq!(eleven.family, ReleaseWindowsFamily::Windows11);

    let mut relabeled = ten;
    relabeled.family = ReleaseWindowsFamily::Windows11;
    assert!(relabeled.verify().is_err());
    assert!(!Rt64PlatformTarget::Windows11D3d12
        .matches_host(ReleaseHostPlatform::WindowsX86_64, Some(relabeled),));

    let server = serde_json::json!({
        "family": "windows10",
        "major": 10,
        "minor": 0,
        "build": 19045,
        "update_build_revision": 6456,
        "product_type": "server"
    });
    assert!(serde_json::from_value::<ReleaseWindowsVersionEvidence>(server).is_err());
}

#[test]
fn opaque_fixture_rejects_target_host_mismatch_and_tamper() {
    assert!(VerifiedRt64PlatformCaseSeries::fixture_for_test(
        Rt64PlatformTarget::Windows11Vulkan,
        Rt64PlatformCase::ResolutionDownsample,
        (ReleaseHostPlatform::WindowsX86_64, Some(windows(22_000)),),
        ("windows11-vulkan-report", bound_report(1), bound_events(1)),
        1,
    )
    .is_ok());
    assert!(matches!(
        VerifiedRt64PlatformCaseSeries::fixture_for_test(
            Rt64PlatformTarget::Windows10Vulkan,
            Rt64PlatformCase::ResolutionDownsample,
            (ReleaseHostPlatform::WindowsX86_64, Some(windows(22_000)),),
            ("windows10-vulkan-report", bound_report(2), bound_events(2)),
            2,
        ),
        Err(PlatformCertificationError::TargetHostMismatch)
    ));

    let series = VerifiedRt64PlatformCaseSeries::fixture_for_test(
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::BackendLifecycle,
        (ReleaseHostPlatform::MacosArm64, None),
        ("macos-metal-report", bound_report(3), bound_events(3)),
        3,
    )
    .unwrap();
    let mut retained = series.revalidate_for_release_matrix().unwrap();
    retained.capture_api = "caller-label".to_owned();
    assert_eq!(
        retained.verify_integrity(),
        Err(PlatformCertificationError::TargetApiMismatch)
    );
}

#[test]
fn authority_requires_exact_repeat_bar_and_unique_events() {
    let series = VerifiedRt64PlatformCaseSeries::fixture_for_test(
        Rt64PlatformTarget::LinuxVulkan,
        Rt64PlatformCase::DeferredDebugger,
        (ReleaseHostPlatform::LinuxX86_64, None),
        ("linux-vulkan-report", bound_report(4), bound_events(4)),
        4,
    )
    .unwrap();
    let mut retained = series.revalidate_for_release_matrix().unwrap();
    retained.run_event_sha256s.pop();
    retained.authority_sha256 = retained.recompute_authority_sha256();
    assert!(matches!(
        retained.verify_integrity(),
        Err(PlatformCertificationError::WrongRunCount {
            expected: 20,
            observed: 19
        })
    ));
}

#[test]
fn typed_target_and_case_denominator_matches_the_project_catalog() {
    let catalog: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../docs/rt64-platform-certification.json"
    ))
    .unwrap();
    let targets = catalog["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| {
            (
                target["id"].as_str().unwrap(),
                target["os_family"].as_str().unwrap(),
                target["graphics_api"].as_str().unwrap(),
                target["capture_api"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        Rt64PlatformTarget::ALL
            .iter()
            .map(|target| {
                (
                    target.id(),
                    target.os_family_id(),
                    match target.graphics_api() {
                        ReleaseGraphicsApi::D3d12 => "d3d12",
                        ReleaseGraphicsApi::Vulkan => "vulkan",
                        ReleaseGraphicsApi::Metal => "metal",
                    },
                    target.capture_api(),
                )
            })
            .collect::<Vec<_>>()
    );

    let cases = catalog["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["id"].as_str().unwrap(),
                case["example"].as_str().unwrap(),
                case["features"].as_array().unwrap()[0].as_str().unwrap(),
                case["repeat_bar"].as_u64().unwrap() as usize,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cases,
        Rt64PlatformCase::ALL
            .iter()
            .map(|case| (
                case.id(),
                case.example(),
                case.features(),
                case.repeat_bar()
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn child_identity_requires_one_pinned_authoritative_target_envelope() {
    let identity = Rt64PlatformChildIdentity {
        schema: RT64_PLATFORM_CHILD_IDENTITY_SCHEMA.to_owned(),
        rt64_source_id: PINNED_RT64_SOURCE_ID.to_owned(),
        source_authoritative: true,
        adapter_source_sha256: "ab".repeat(32),
        capture_api: Rt64PlatformTarget::MacosMetal.capture_api().to_owned(),
    };
    let stdout = format!(
        "semantic evidence\n{CHILD_IDENTITY_PREFIX}{}\n",
        serde_json::to_string(&identity).unwrap()
    );
    assert_eq!(
        parse_child_identity(
            stdout.as_bytes(),
            Rt64PlatformTarget::MacosMetal,
            &"ab".repeat(32),
        )
        .unwrap(),
        identity
    );

    let mut forged = identity.clone();
    forged.source_authoritative = false;
    let forged_stdout = format!(
        "{CHILD_IDENTITY_PREFIX}{}\n",
        serde_json::to_string(&forged).unwrap()
    );
    assert_eq!(
        parse_child_identity(
            forged_stdout.as_bytes(),
            Rt64PlatformTarget::MacosMetal,
            &"ab".repeat(32),
        ),
        Err(PlatformCertificationError::SourceMismatch)
    );
    assert!(matches!(
        parse_child_identity(
            stdout.as_bytes(),
            Rt64PlatformTarget::LinuxVulkan,
            &"ab".repeat(32),
        ),
        Err(PlatformCertificationError::TargetApiMismatch)
    ));
    assert_eq!(
        parse_child_identity(
            stdout.as_bytes(),
            Rt64PlatformTarget::MacosMetal,
            &"cd".repeat(32),
        ),
        Err(PlatformCertificationError::AdapterSourceMismatch)
    );
    assert!(parse_child_identity(
        format!("{stdout}{stdout}").as_bytes(),
        Rt64PlatformTarget::MacosMetal,
        &"ab".repeat(32),
    )
    .is_err());
}

#[test]
fn case_run_events_bind_target_case_report_child_and_ordinal() {
    let nonce = [7; 32];
    let first = derive_case_run_event(
        &nonce,
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::ResolutionDownsample,
        &"11".repeat(32),
        &"22".repeat(32),
        1,
    );
    let second = derive_case_run_event(
        &nonce,
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::ResolutionDownsample,
        &"11".repeat(32),
        &"22".repeat(32),
        2,
    );
    let other_case = derive_case_run_event(
        &nonce,
        Rt64PlatformTarget::MacosMetal,
        Rt64PlatformCase::FramebufferEnhancement,
        &"11".repeat(32),
        &"22".repeat(32),
        1,
    );
    assert!(canonical_sha256(&first));
    assert_ne!(first, second);
    assert_ne!(first, other_case);
}

#[test]
fn embedded_cargo_identity_revalidates() {
    assert!(verified_build_cargo().unwrap().is_file());
}

#[test]
fn platform_binding_preflight_rejects_stale_adapter_source() {
    let current = "ab".repeat(32);
    let stale = rt64_renderer(Rt64PlatformTarget::MacosMetal, &"cd".repeat(32));
    assert_eq!(
        validate_report_binding(
            Rt64PlatformTarget::MacosMetal,
            (ReleaseHostPlatform::MacosArm64, None),
            ReleaseHostPlatform::MacosArm64,
            None,
            &stale,
            &current,
        ),
        Err(PlatformCertificationError::AdapterSourceMismatch)
    );

    let matching = rt64_renderer(Rt64PlatformTarget::MacosMetal, &current);
    assert_eq!(
        validate_report_binding(
            Rt64PlatformTarget::MacosMetal,
            (ReleaseHostPlatform::MacosArm64, None),
            ReleaseHostPlatform::MacosArm64,
            None,
            &matching,
            &current,
        ),
        Ok(())
    );
}

#[test]
fn platform_binding_preflight_requires_exact_windows_host_evidence() {
    let target = Rt64PlatformTarget::Windows11D3d12;
    let adapter_source_sha256 = "ab".repeat(32);
    let renderer = rt64_renderer(target, &adapter_source_sha256);
    let current = windows(26100);
    let retained = windows(22631);
    assert_eq!(current.family, retained.family);
    assert_eq!(
        validate_report_binding(
            target,
            (ReleaseHostPlatform::WindowsX86_64, Some(current)),
            ReleaseHostPlatform::WindowsX86_64,
            Some(retained),
            &renderer,
            &adapter_source_sha256,
        ),
        Err(PlatformCertificationError::TargetHostMismatch)
    );
    assert_eq!(
        validate_report_binding(
            target,
            (ReleaseHostPlatform::WindowsX86_64, Some(current)),
            ReleaseHostPlatform::WindowsX86_64,
            Some(current),
            &renderer,
            &adapter_source_sha256,
        ),
        Ok(())
    );
}
