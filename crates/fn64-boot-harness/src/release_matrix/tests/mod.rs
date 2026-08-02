
use super::*;
use crate::{
    load_private_release_run_contract, materialize_release_program_build_receipt,
    parse_unsupported_journal, run_private_release_series, verify_private_release_series,
    ReleaseProgramBuildReceiptInput,
};
use crate::{
    ArtifactKind, ClosurePath, ClosurePathStatus, FixedCycleDigestGate, LiveRenderEvidence,
    RenderPixelFormat, RspRdpObservationEventEvidence, LIVE_MINIMUM_CLOSURE_PATHS,
};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static PRODUCTION_MATRIX_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ProductionMatrixFixtureDirectory(PathBuf);

impl ProductionMatrixFixtureDirectory {
    fn new() -> Self {
        let counter = PRODUCTION_MATRIX_FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = if Path::new("/private/tmp").is_dir() {
            PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        };
        let path = base.join(format!(
            "fn64-production-matrix-fixture-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for ProductionMatrixFixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const CLEAN_RT64_IDENTITY: &str = concat!(
    "adapter=fn64-render-rt64/rt64;adapter_sha256=",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ";source=git:",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ";provenance=git-clean;overlay=fn64-test;post_vi_api=vulkan-bgra8-rgba8-unorm"
);

fn clean_rt64_identity_for(api: ReleaseGraphicsApi) -> String {
    let post_vi_api = match api {
        ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
        ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
        ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
    };
    format!(
        "adapter=fn64-render-rt64/rt64;adapter_sha256={};source=git:{};provenance=git-clean;overlay=fn64-test;post_vi_api={post_vi_api}",
        "aa".repeat(32),
        "bb".repeat(20),
    )
}

fn closed_report(
    scenario: &str,
    input: &[u8],
    framebuffer_byte: u8,
    feature_path: &str,
    rt64_identity: &str,
    program: Option<ProgramFeature>,
) -> ReleaseGateReport {
    closed_report_with_rt64_environment(
        scenario,
        input,
        framebuffer_byte,
        feature_path,
        rt64_identity,
        program,
        ReleaseHostPlatform::LinuxX86_64,
        ReleaseGraphicsApi::Vulkan,
    )
}

#[allow(clippy::too_many_arguments)]
fn closed_report_with_rt64_environment(
    scenario: &str,
    input: &[u8],
    framebuffer_byte: u8,
    feature_path: &str,
    rt64_identity: &str,
    program: Option<ProgramFeature>,
    rt64_platform: ReleaseHostPlatform,
    rt64_graphics_api: ReleaseGraphicsApi,
) -> ReleaseGateReport {
    let (observations, framebuffer_artifact) = if scenario.contains("rt64") {
        let render = LiveRenderEvidence::post_vi_swapchain(
            100,
            rt64_identity,
            [0x11; 32],
            1,
            1,
            4,
            RenderPixelFormat::Bgra8Unorm,
            1,
            1,
            vec![framebuffer_byte; 4],
        )
        .unwrap();
        (
            ReleaseObservationGeometry::post_vi_swapchain(
                rt64_identity,
                "11".repeat(32),
                1,
                1,
                1,
                1,
                4,
                4,
            )
            .unwrap(),
            render.canonical_bytes(),
        )
    } else {
        (
            ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            vec![framebuffer_byte; 2],
        )
    };
    let mut digest = FixedCycleDigestGate::new(100);
    digest
        .capture(100, ArtifactKind::Framebuffer, &framebuffer_artifact)
        .unwrap();
    for kind in [
        ArtifactKind::Audio,
        ArtifactKind::DeviceState,
        ArtifactKind::TimingTrace,
    ] {
        digest.capture(100, kind, &[kind as u8]).unwrap();
    }
    digest
        .capture(
            100,
            ArtifactKind::Memory,
            &vec![0; crate::DEFAULT_RDRAM_SIZE],
        )
        .unwrap();
    let mut closure: Vec<_> = LIVE_MINIMUM_CLOSURE_PATHS
        .iter()
        .map(|name| ClosurePath {
            name: (*name).to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        })
        .collect();
    closure.push(ClosurePath {
        name: feature_path.to_owned(),
        observations: 1,
        status: ClosurePathStatus::ExercisedZeroUnsupported,
        unsupported: Vec::new(),
    });
    let is_rt64 = scenario.contains("rt64");
    for path in if is_rt64 {
        [
            Some("controller.standard-input-read"),
            Some("controller.rumble-operation"),
        ]
    } else {
        [Some("controller.standard-input-read"), None]
    }
    .into_iter()
    .flatten()
    {
        closure.push(ClosurePath {
            name: path.to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        });
    }
    let environment = ReleaseEnvironmentEvidence {
        platform: if is_rt64 {
            rt64_platform
        } else {
            ReleaseHostPlatform::MacosArm64
        },
        windows_version: (is_rt64 && rt64_platform == ReleaseHostPlatform::WindowsX86_64).then(
            || {
                crate::ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 19_045, 1)
                    .unwrap()
            },
        ),
        controller_ports: [
            if is_rt64 {
                ReleaseControllerPort::StandardControllerRumblePak
            } else {
                ReleaseControllerPort::StandardControllerNoPak
            },
            ReleaseControllerPort::Absent,
            ReleaseControllerPort::Absent,
            ReleaseControllerPort::Absent,
        ],
        cartridge_save: if feature_path == "save.sram-operation" {
            ReleaseCartridgeSave::Sram32Kib
        } else {
            ReleaseCartridgeSave::Eeprom4k
        },
        audio_task_execution: crate::ReleaseAudioTaskExecutionPolicy::LleAccuracy,
        renderer: if is_rt64 {
            ReleaseRendererEvidence::Rt64 {
                execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                tv_type: crate::ReleaseTvStandard::Ntsc,
                graphics_api: rt64_graphics_api,
                backend_identity: rt64_identity.to_owned(),
                source_authoritative: true,
                settings_sha256: "11".repeat(32),
                replacement_packs_active: false,
            }
        } else {
            ReleaseRendererEvidence::Reference {
                execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                tv_type: crate::ReleaseTvStandard::Ntsc,
            }
        },
    };
    let execution_destinations = match program {
        Some(ProgramFeature::NativeArchive) => ExecutionDestinationEvidence::from_ordered(
            crate::ExecutionDestinationSource::NativeArchive {
                artifact_sha256: "aa".repeat(32),
            },
            vec![crate::ExecutionDestinationEventEvidence {
                guest_cycle: Some(1),
                destination: crate::ReleaseExecutionDestination::Native {
                    section_index: 0,
                    function_offset: 0x1000,
                    link_vram: 0x8000_1000,
                },
            }],
        )
        .unwrap(),
        Some(ProgramFeature::TypedObservedFunction) => {
            ExecutionDestinationEvidence::from_ordered(
                crate::ExecutionDestinationSource::TypedObservedFunctionProgram {
                    artifact_sha256: "cc".repeat(32),
                },
                vec![crate::ExecutionDestinationEventEvidence {
                    guest_cycle: Some(1),
                    destination: crate::ReleaseExecutionDestination::TypedFunction {
                        vram: 0x8000_1000,
                        symbol: "entry".to_owned(),
                    },
                }],
            )
            .unwrap()
        }
        Some(ProgramFeature::TypedBlock) => ExecutionDestinationEvidence::from_ordered(
            crate::ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: "dd".repeat(32),
                dispatch_artifact_sha256: "ee".repeat(32),
            },
            vec![crate::ExecutionDestinationEventEvidence {
                guest_cycle: None,
                destination: crate::ReleaseExecutionDestination::TypedBlock {
                    bank: 1,
                    pc: 0x8000_1000,
                    runner_artifact_sha256: "ff".repeat(32),
                },
            }],
        )
        .unwrap(),
        None => ExecutionDestinationEvidence::no_program(),
    };
    ReleaseGateReport::new_with_test_environment_and_destinations(
        scenario,
        input,
        digest.finish().unwrap(),
        observations,
        environment,
        execution_destinations,
        closure,
    )
    .unwrap()
}

fn with_rsp_rdp_observations(
    mut report: ReleaseGateReport,
    ordered: Vec<RspRdpObservationEventEvidence>,
) -> ReleaseGateReport {
    report.rsp_rdp = RspRdpEvidence::from_ordered(ordered).unwrap();
    report.report_sha256 = hex(&Sha256::digest(
        crate::release_gate::encode_report_evidence(&report).unwrap(),
    ));
    report.verify_integrity().unwrap();
    report
}

fn scenario(id: &str, report: &ReleaseGateReport) -> ReleaseMatrixScenario {
    let mut scenario = ReleaseMatrixScenario {
        id: id.to_owned(),
        report_scenario: report.scenario.clone(),
        input_sha256: report.input_sha256.clone(),
        report_sha256: report.report_sha256.clone(),
        declaration_sha256: String::new(),
    };
    scenario.declaration_sha256 = scenario.recompute_declaration_sha256();
    scenario
}

fn evidence_series(
    report: ReleaseGateReport,
) -> Vec<(ReleaseGateReport, ParsedUnsupportedJournal)> {
    (0..RELEASE_MATRIX_REPORT_COUNT)
        .map(|index| {
            let journal = ParsedUnsupportedJournal {
                events: Vec::new(),
                completion: crate::UnsupportedJournalCompletion::V3RunBound {
                    guest_cycle: report.digest.guest_cycle,
                    report_sha256: report.report_sha256.clone(),
                    run_event_sha256: hex(&Sha256::digest(format!(
                        "{}:{index}",
                        report.report_sha256
                    ))),
                },
            };
            (report.clone(), journal)
        })
        .collect()
}

fn fixture() -> (
    ReleaseMatrixManifest,
    Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>,
) {
    let reference = closed_report(
        "game-a-reference",
        b"private-a",
        0xa1,
        "save.eeprom-4k-operation",
        CLEAN_RT64_IDENTITY,
        Some(ProgramFeature::TypedObservedFunction),
    );
    let rt64 = closed_report(
        "game-b-rt64",
        b"private-b",
        0xb2,
        "save.sram-operation",
        CLEAN_RT64_IDENTITY,
        Some(ProgramFeature::TypedObservedFunction),
    );
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![
            scenario("reference-evidence", &reference),
            scenario("rt64-evidence", &rt64),
        ],
    };
    let reference = evidence_series(reference);
    let rt64 = evidence_series(rt64);
    let mut evidence = Vec::with_capacity(RELEASE_MATRIX_REPORT_COUNT * 2);
    for index in 0..RELEASE_MATRIX_REPORT_COUNT {
        // Deliberately interleave the flat input in the opposite order
        // from the manifest. Routing authority is report.scenario.
        evidence.push(rt64[index].clone());
        evidence.push(reference[index].clone());
    }
    (manifest, evidence)
}

fn incomplete_fixture() -> (
    ReleaseMatrixManifest,
    Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>,
    IncompleteReleaseMatrix,
) {
    let (manifest, evidence) = fixture();
    let incomplete = match verify_release_matrix(&manifest, &evidence).unwrap() {
        ReleaseMatrixVerification::Incomplete(incomplete) => incomplete,
        ReleaseMatrixVerification::Complete(_) => {
            panic!("two scenarios cannot cover the fixed full-parity denominator")
        }
    };
    incomplete.verify_integrity().unwrap();
    (manifest, evidence, incomplete)
}

fn rt64_report_for_platform_api(
    scenario_name: &str,
    input: &[u8],
    platform: ReleaseHostPlatform,
    graphics_api: ReleaseGraphicsApi,
) -> ReleaseGateReport {
    let identity = clean_rt64_identity_for(graphics_api);
    closed_report_with_rt64_environment(
        scenario_name,
        input,
        0xc3,
        "save.sram-operation",
        &identity,
        Some(ProgramFeature::TypedObservedFunction),
        platform,
        graphics_api,
    )
}

fn incomplete_for_report(id: &str, report: ReleaseGateReport) -> IncompleteReleaseMatrix {
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario(id, &report)],
    };
    match verify_release_matrix(&manifest, &evidence_series(report)).unwrap() {
        ReleaseMatrixVerification::Incomplete(incomplete) => incomplete,
        ReleaseMatrixVerification::Complete(_) => {
            panic!("one report series cannot cover the fixed full-parity denominator")
        }
    }
}

fn with_rom(
    mut report: ReleaseGateReport,
    destination_code: u8,
    class: crate::ReleaseRomClass,
) -> ReleaseGateReport {
    let mut bytes = vec![0; 0x1000];
    bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    bytes[0x3b..0x3f].copy_from_slice(&[b'N', b'F', b'6', destination_code]);
    let tv_type = crate::ReleaseRomEvidence::decode_tv_type(&bytes)
        .unwrap()
        .unwrap_or(fn64_runtime::TvType::Ntsc);
    let tv_standard = crate::ReleaseTvStandard::from(tv_type);
    match &mut report.environment.renderer {
        ReleaseRendererEvidence::Reference { tv_type, .. }
        | ReleaseRendererEvidence::Rt64 { tv_type, .. } => *tv_type = tv_standard,
    }
    report.input_sha256 = hex(&Sha256::digest(&bytes));
    report.rom = Some(
        crate::ReleaseRomEvidence::from_bytes(&bytes, class, tv_type)
            .expect("test ROM header is valid"),
    );
    report.report_sha256 = hex(&Sha256::digest(
        crate::release_gate::encode_report_evidence(&report)
            .expect("test report evidence encodes"),
    ));
    report.verify_integrity().unwrap();
    report
}

fn assigned_requirement_ids(
    incomplete: &IncompleteReleaseMatrix,
    class: CertificationRequirementClass,
) -> BTreeSet<String> {
    incomplete
        .satisfied
        .iter()
        .filter(|assignment| assignment.requirement.class() == class)
        .map(|assignment| assignment.requirement.id().to_owned())
        .collect()
}

fn rom_class_authority(report: &ReleaseGateReport) -> VerifiedRomClassAuthority {
    let rom = report
        .rom
        .as_ref()
        .expect("authority fixture has ROM evidence");
    let mut authority = VerifiedRomClassAuthority {
        schema: VERIFIED_ROM_CLASS_AUTHORITY_SCHEMA.to_owned(),
        contract_schema: crate::PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
        contract_sha256: "91".repeat(32),
        receipt_schema: crate::PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA.to_owned(),
        receipt_sha256: "93".repeat(32),
        runner_executable_sha256: "94".repeat(32),
        purpose: "full_rom".to_owned(),
        report_scenario: report.scenario.clone(),
        input_sha256: report.input_sha256.clone(),
        input_bytes: rom.byte_len,
        rom_class: rom.class,
        guest_cycle: report.digest.guest_cycle,
        expected_execution_source: report.execution_destinations.source.clone(),
        child_executable_sha256: "92".repeat(32),
        semantic_report_sha256: report.report_sha256.clone(),
        run_event_sha256s: evidence_series(report.clone())
            .into_iter()
            .map(|(_, journal)| {
                journal
                    .release_run_event_sha256()
                    .expect("test journal has a run event")
                    .to_owned()
            })
            .collect(),
        authority_sha256: String::new(),
    };
    authority.authority_sha256 = authority.recompute_authority_sha256();
    authority
}

fn replace_report(
    manifest: &mut ReleaseMatrixManifest,
    evidence: &mut Vec<(ReleaseGateReport, ParsedUnsupportedJournal)>,
    report: ReleaseGateReport,
) {
    let declaration = manifest
        .scenarios
        .iter_mut()
        .find(|scenario| scenario.report_scenario == report.scenario)
        .expect("replacement report scenario is declared");
    declaration.input_sha256 = report.input_sha256.clone();
    declaration.report_sha256 = report.report_sha256.clone();
    declaration.declaration_sha256 = declaration.recompute_declaration_sha256();
    evidence.retain(|(existing, _)| existing.scenario != report.scenario);
    evidence.extend(evidence_series(report));
}

fn forged_ref(class: CertificationRequirementClass, id: &str) -> CertificationRequirementRef {
    serde_json::from_value(serde_json::json!({
        "class": class,
        "id": id,
    }))
    .unwrap()
}

fn requirement_keys(
    requirements: impl IntoIterator<Item = CertificationRequirementRef>,
) -> Vec<(CertificationRequirementClass, String)> {
    requirements
        .into_iter()
        .map(|requirement| (requirement.class(), requirement.id().to_owned()))
        .collect()
}

fn profile_keys() -> Vec<(CertificationRequirementClass, String)> {
    FullParityV1::new()
        .requirements()
        .into_iter()
        .map(|requirement| (requirement.class(), requirement.id().to_owned()))
        .collect()
}

fn platform_case_fixture(
    report: &ReleaseGateReport,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    target: Rt64PlatformTarget,
    case: Rt64PlatformCase,
    seed: u8,
) -> VerifiedRt64PlatformCaseSeries {
    let verified = verify_release_evidence_series(evidence, RELEASE_MATRIX_REPORT_COUNT)
        .expect("matrix fixture series is valid");
    VerifiedRt64PlatformCaseSeries::fixture_for_test(
        target,
        case,
        (
            report.environment.platform,
            report.environment.windows_version,
        ),
        (
            &report.scenario,
            report.report_sha256.clone(),
            verified.run_event_sha256s,
        ),
        seed,
    )
    .unwrap()
}

fn pinned_platform_identity(api: ReleaseGraphicsApi, adapter_sha256: &str) -> String {
    format!(
        "adapter=fn64-render-rt64/rt64;adapter_sha256={adapter_sha256};source=git:f0728a2520d5aa735886240de3fee75cc805f6d6;provenance=git-clean;overlay=fn64-test;post_vi_api={}",
        match api {
            ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
        }
    )
}

mod part1;
mod part2;
