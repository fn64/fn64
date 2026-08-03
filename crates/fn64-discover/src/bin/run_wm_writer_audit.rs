//! Guarded private WM selected-build writer audit.
//!
//! This binary deliberately keeps the verified build, writer-audit session,
//! and sealed bundle in one process. Only pointer-free projections are written
//! after the independent exact-ten series run. Complete denominator receipts
//! still require all eight series; a failed run retains a distinct diagnostic
//! receipt for successful series without making it scorecard authority.

// Everything below main() is live only under the writer-runtime-authority
// feature; the default build compiles it dead and would warn on every item.
#![cfg_attr(not(feature = "writer-runtime-authority"), allow(dead_code))]

use std::path::{Component, Path, PathBuf};

const OUTPUT_WRITER_DENOMINATOR: &str = "writers.json";
const OUTPUT_AUDIT_RECEIPT: &str = "writer-audit.json";
const OUTPUT_PARTIAL_AUDIT_RECEIPT: &str = "partial-writer-audit.json";
const OUTPUT_PARTIAL_WRITER_DENOMINATOR: &str = "partial-writers.json";
const FAILURE_DIAGNOSTIC_LIMIT: usize = 64 * 1024;
const DEFAULT_MAX_BUILD_SECONDS: u64 = 7_200;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CliImageGroup {
    name: String,
    captures: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CliInputs {
    rom: PathBuf,
    boot_context: PathBuf,
    image_groups: Vec<CliImageGroup>,
    output: PathBuf,
    max_build_seconds: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuditSeries {
    Bootstrap,
    Cpu,
    HostAbi,
    Pi,
    RdpRenderer,
    Rsp,
    Si,
    Sp,
}

impl AuditSeries {
    const fn token(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Cpu => "cpu",
            Self::HostAbi => "host_abi",
            Self::Pi => "pi",
            Self::RdpRenderer => "rdp_renderer",
            Self::Rsp => "rsp",
            Self::Si => "si",
            Self::Sp => "sp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeriesProgress {
    Started,
    Completed,
    Failed,
}

const ALL_AUDIT_SERIES: [AuditSeries; 8] = [
    AuditSeries::Bootstrap,
    AuditSeries::Cpu,
    AuditSeries::HostAbi,
    AuditSeries::Pi,
    AuditSeries::RdpRenderer,
    AuditSeries::Rsp,
    AuditSeries::Si,
    AuditSeries::Sp,
];

trait WriterAuditSession {
    type Bundle;

    fn run_series(&mut self, series: AuditSeries) -> Result<(), String>;
    fn seal(self) -> Result<Self::Bundle, String>;
}

struct AuditRun<B> {
    bundle: Option<B>,
    failures: Vec<(AuditSeries, String)>,
}

fn bounded_failure_diagnostic(error: &str) -> (&[u8], bool) {
    let bytes = error.as_bytes();
    let retained = &bytes[..bytes.len().min(FAILURE_DIAGNOSTIC_LIMIT)];
    (retained, retained.len() != bytes.len())
}

fn run_all_series_with_progress<S: WriterAuditSession>(
    mut session: S,
    mut progress: impl FnMut(AuditSeries, SeriesProgress),
) -> Result<AuditRun<S::Bundle>, String> {
    let mut completed = 0usize;
    let mut failures = Vec::new();
    for series in ALL_AUDIT_SERIES {
        progress(series, SeriesProgress::Started);
        match session.run_series(series) {
            Ok(()) => {
                completed += 1;
                progress(series, SeriesProgress::Completed);
            }
            Err(error) => {
                progress(series, SeriesProgress::Failed);
                failures.push((series, error));
            }
        }
    }
    let bundle = if completed == 0 {
        None
    } else {
        Some(session.seal()?)
    };
    Ok(AuditRun { bundle, failures })
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<CliInputs, String> {
    let mut args = args.into_iter().peekable();
    let mut rom = None;
    let mut boot_context = None;
    let mut image_groups = Vec::new();
    let mut output = None;
    let mut max_build_seconds = DEFAULT_MAX_BUILD_SECONDS;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--rom" => set_once_path(&mut rom, next_value(&mut args, "--rom")?, "--rom")?,
            "--boot-context" => set_once_path(
                &mut boot_context,
                next_value(&mut args, "--boot-context")?,
                "--boot-context",
            )?,
            "--output" => {
                set_once_path(&mut output, next_value(&mut args, "--output")?, "--output")?
            }
            "--max-build-seconds" => {
                let value = next_value(&mut args, "--max-build-seconds")?;
                max_build_seconds = value
                    .parse()
                    .map_err(|_| "--max-build-seconds must be an integer".to_owned())?;
            }
            "--image-group" => {
                let name = next_value(&mut args, "--image-group name")?;
                let mut captures = Vec::new();
                while args.peek().is_some_and(|value| !value.starts_with("--")) {
                    captures.push(PathBuf::from(
                        args.next().expect("peeked argument vanished"),
                    ));
                }
                if captures.len() < 3 {
                    return Err("each --image-group requires at least three capture paths".into());
                }
                image_groups.push(CliImageGroup { name, captures });
            }
            "-h" | "--help" => return Err(usage()),
            _ => return Err("unknown argument\n".to_owned() + &usage()),
        }
    }
    let parsed = CliInputs {
        rom: rom.ok_or_else(|| "missing --rom".to_owned())?,
        boot_context: boot_context.ok_or_else(|| "missing --boot-context".to_owned())?,
        image_groups,
        output: output.ok_or_else(|| "missing --output".to_owned())?,
        max_build_seconds,
    };
    validate_cli_inputs(&parsed)?;
    Ok(parsed)
}

fn next_value(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    option: &str,
) -> Result<String, String> {
    args.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{option} requires a value"))
}

fn set_once_path(slot: &mut Option<PathBuf>, value: String, option: &str) -> Result<(), String> {
    if slot.replace(PathBuf::from(value)).is_some() {
        return Err(format!("{option} may be supplied only once"));
    }
    Ok(())
}

fn validate_cli_inputs(inputs: &CliInputs) -> Result<(), String> {
    for (path, label) in [
        (&inputs.rom, "--rom"),
        (&inputs.boot_context, "--boot-context"),
        (&inputs.output, "--output"),
    ] {
        validate_absolute_path(path, label)?;
    }
    if inputs.image_groups.is_empty() {
        return Err("at least one --image-group is required".into());
    }
    let mut group_names = std::collections::BTreeSet::new();
    for group in &inputs.image_groups {
        if !group.name.starts_with("FN64_EXECUTABLE_IMAGE_")
            || !group
                .name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("image-group names must be FN64_EXECUTABLE_IMAGE_* tokens".into());
        }
        if !group_names.insert(&group.name) {
            return Err("image-group names must be unique".into());
        }
        for capture in &group.captures {
            validate_absolute_path(capture, "image capture")?;
        }
    }
    if !(2_400..=7_200).contains(&inputs.max_build_seconds) {
        return Err("--max-build-seconds must be 2400..=7200".into());
    }
    Ok(())
}

fn validate_absolute_path(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("{label} must be an absolute path without '..'"));
    }
    Ok(())
}

fn usage() -> String {
    "usage: run_wm_writer_audit --rom ABS --boot-context ABS \\
       --image-group FN64_EXECUTABLE_IMAGE_NAME ABS ABS ABS [ABS ...] \\
       [--image-group ...] --output ABS_NEW_DIRECTORY \\
       [--max-build-seconds 2400..7200]"
        .into()
}

#[cfg(feature = "writer-runtime-authority")]
mod production {
    use super::*;
    use fn64_boot_harness::{
        build_wm2000_generated_runner_v1, GeneratedRunnerWriterAuditSessionV1,
        Wm2000ExecutableImageGroupV1, Wm2000GeneratedRunnerBuildInputsV1,
        WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1, WRITER_AUDIT_CPU_COMPLETED_V1,
        WRITER_AUDIT_HOST_ABI_COMPLETED_V1, WRITER_AUDIT_PI_COMPLETED_V1,
        WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1, WRITER_AUDIT_RSP_COMPLETED_V1,
        WRITER_AUDIT_SI_COMPLETED_V1, WRITER_AUDIT_SP_COMPLETED_V1,
    };
    use fn64_discover::writer_denominator::{
        OpenWriterChannelInputV2, WriterChannelBlockerCodeV2, WriterChannelBlockerV2,
        WriterChannelDenominatorInputV2, WriterChannelDenominatorV2, WRITER_CHANNELS_V2,
    };
    use serde::Serialize;
    use sha2::{Digest, Sha256};
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write};
    use std::time::Instant;

    const ALL_CHANNEL_BITS: u8 = WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
        | WRITER_AUDIT_CPU_COMPLETED_V1
        | WRITER_AUDIT_HOST_ABI_COMPLETED_V1
        | WRITER_AUDIT_PI_COMPLETED_V1
        | WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
        | WRITER_AUDIT_RSP_COMPLETED_V1
        | WRITER_AUDIT_SI_COMPLETED_V1
        | WRITER_AUDIT_SP_COMPLETED_V1;

    fn print_progress(fields: impl std::fmt::Display) {
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "writer-progress mode=authority {fields}")
            .and_then(|()| stdout.flush());
    }

    impl WriterAuditSession for GeneratedRunnerWriterAuditSessionV1 {
        type Bundle = fn64_boot_harness::VerifiedGeneratedRunnerWriterAuditBundleV1;

        fn run_series(&mut self, series: AuditSeries) -> Result<(), String> {
            let result = match series {
                AuditSeries::Bootstrap => self.run_bootstrap_runtime_series_v1(),
                AuditSeries::Cpu => self.run_cpu_runtime_series_v1(),
                AuditSeries::HostAbi => self.run_host_abi_runtime_series_v1(),
                AuditSeries::Pi => self.run_pi_runtime_series_v1(),
                AuditSeries::RdpRenderer => self.run_rdp_renderer_runtime_series_v1(),
                AuditSeries::Rsp => self.run_rsp_runtime_series_v1(),
                AuditSeries::Si => self.run_si_runtime_series_v1(),
                AuditSeries::Sp => self.run_sp_runtime_series_v1(),
            };
            result.map_err(|error| format!("selected-build writer series failed: {error}"))
        }

        fn seal(self) -> Result<Self::Bundle, String> {
            GeneratedRunnerWriterAuditSessionV1::seal(self)
                .map_err(|error| format!("seal writer-audit session: {error}"))
        }
    }

    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct PathFreeAuditReceiptV1<'a> {
        schema: &'static str,
        exact_runs_per_channel: u8,
        channel_count: u8,
        completed_channel_bitmap: u8,
        build_schema: &'a str,
        build_authority_sha256: &'a str,
        selected_binary_sha256: &'a str,
        private_build_inputs_sha256: &'a str,
        cargo_graph_sha256: &'a str,
        cargo_source_sha256: &'a str,
        build_environment_sha256: &'a str,
        builder_cargo_sha256: &'a str,
        builder_rustc_sha256: &'a str,
        cargo_config_sha256: &'a str,
        memory_guard_sha256: &'a str,
        selected_build_cargo_jobs: u16,
        build_max_rss_mib: u32,
        build_min_free_percent: u8,
        program_identity_sha256: &'a str,
        normalized_rom_sha256: &'a str,
        manifest_sha256: &'a str,
        lock_sha256: &'a str,
        root_adapter_source_sha256: &'a str,
        shard_cargo_source_tree_sha256: &'a str,
        emitter_source_sha256: &'a str,
        runtime_source_sha256: &'a str,
        prepared_tree_sha256: &'a str,
        producer_cargo_source_sha256: &'a str,
        producer_binary_sha256: &'a str,
        bundle_schema: &'a str,
        bundle_authority_sha256: &'a str,
        program_model_sha256: &'a str,
        writer_denominator_sha256: &'a str,
    }

    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct PartialSeriesReferenceV1 {
        channel: &'static str,
        series_schema: String,
        series_authority_sha256_reference: String,
    }

    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct PartialFailureDiagnosticV1 {
        channel: &'static str,
        file: String,
        total_bytes: u64,
        retained_bytes: u64,
        truncated: bool,
        sha256: String,
    }

    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct PartialAuditReceiptV1<'a> {
        schema: &'static str,
        status: &'static str,
        exact_runs_per_successful_channel: u8,
        attempted_channel_count: u8,
        completed_channel_bitmap: u8,
        build_schema: &'a str,
        build_authority_sha256: &'a str,
        selected_binary_sha256: &'a str,
        private_build_inputs_sha256: &'a str,
        bundle_schema: Option<String>,
        bundle_authority_sha256_reference: Option<String>,
        program_model_sha256: Option<String>,
        partial_writer_denominator: Option<&'static str>,
        partial_writer_denominator_sha256: Option<String>,
        successful_series_diagnostic_references: Vec<PartialSeriesReferenceV1>,
        failures: &'a [PartialFailureDiagnosticV1],
    }

    pub(super) fn run(inputs: CliInputs) -> Result<(), String> {
        let output = create_private_output_directory(&inputs.output)?;
        print_progress("phase=build state=start");
        let build_started = Instant::now();
        let build = build_wm2000_generated_runner_v1(Wm2000GeneratedRunnerBuildInputsV1 {
            rom: inputs.rom,
            boot_context: inputs.boot_context,
            executable_image_groups: inputs
                .image_groups
                .into_iter()
                .map(|group| Wm2000ExecutableImageGroupV1 {
                    environment_name: group.name,
                    captures: group.captures,
                })
                .collect(),
            max_build_seconds: inputs.max_build_seconds,
        })
        .map_err(|error| format!("verified generated-runner build failed: {error}"))?;
        print_progress(format_args!(
            "phase=build state=complete elapsed_ms={}",
            build_started.elapsed().as_millis()
        ));
        let build_evidence = build.evidence().clone();
        if build_evidence.selected_build_cargo_jobs != 2
            || build_evidence.build_max_rss_mib != 4_096
            || build_evidence.build_min_free_percent != 40
        {
            return Err(
                "verified build did not retain the fixed 2-job/4096 MiB/40% build contract".into(),
            );
        }

        let mut series_started = None;
        let audit_run = run_all_series_with_progress(
            GeneratedRunnerWriterAuditSessionV1::new(build),
            |series, state| match state {
                SeriesProgress::Started => {
                    series_started = Some(Instant::now());
                    print_progress(format_args!(
                        "channel={} phase=series runs=10 state=start",
                        series.token()
                    ));
                }
                SeriesProgress::Completed => {
                    let elapsed = series_started
                        .take()
                        .expect("series completion follows its start")
                        .elapsed()
                        .as_millis();
                    print_progress(format_args!(
                        "channel={} phase=series runs=10 state=complete elapsed_ms={elapsed}",
                        series.token()
                    ));
                }
                SeriesProgress::Failed => {
                    let elapsed = series_started
                        .take()
                        .expect("series failure follows its start")
                        .elapsed()
                        .as_millis();
                    print_progress(format_args!(
                        "channel={} phase=series runs=10 state=fail elapsed_ms={elapsed}",
                        series.token()
                    ));
                }
            },
        )?;
        if !audit_run.failures.is_empty() {
            let diagnostics = write_failure_diagnostics(&output, &audit_run.failures)?;
            write_partial_outputs(&output, &build_evidence, audit_run.bundle, &diagnostics)?;
            print_progress(format_args!(
                "phase=audit state=incomplete failed_channels={} partial_receipt={OUTPUT_PARTIAL_AUDIT_RECEIPT}",
                diagnostics.len()
            ));
            return Err(format!(
                "{} writer channel series failed; all eight channels were attempted and diagnostic-only partial evidence was retained",
                diagnostics.len()
            ));
        }
        let bundle = audit_run
            .bundle
            .ok_or_else(|| "writer-audit session completed no channel series".to_owned())?;
        let bundle_evidence = bundle.evidence();
        if bundle_evidence.completed_channels != ALL_CHANNEL_BITS
            || bundle_evidence.bootstrap.is_none()
            || bundle_evidence.cpu.is_none()
            || bundle_evidence.host_abi.is_none()
            || bundle_evidence.pi.is_none()
            || bundle_evidence.rdp_renderer.is_none()
            || bundle_evidence.rsp.is_none()
            || bundle_evidence.si.is_none()
            || bundle_evidence.sp.is_none()
        {
            return Err("writer-audit session did not complete all eight channels".into());
        }
        let program_model_sha256 = bundle_evidence
            .bootstrap
            .as_ref()
            .expect("checked bootstrap series")
            .program_model_sha256
            .clone();
        let bundle_schema = bundle_evidence.schema;
        let bundle_authority_sha256 = bundle_evidence.authority_sha256.clone();

        let denominator = new_open_writer_denominator(program_model_sha256.clone())?
            .complete_writer_audit_bundle(bundle)
            .map_err(|error| format!("consume writer-audit capability: {error}"))?;
        if !denominator.is_complete() || !denominator.open_channels().is_empty() {
            return Err("writer denominator remained incomplete after all eight series".into());
        }
        let denominator_json = denominator
            .canonical_json_bytes()
            .map_err(|error| format!("serialize complete writer denominator: {error}"))?;
        let denominator_sha256 = format!("{:x}", Sha256::digest(&denominator_json));
        let audit_json = serde_json::to_vec(&PathFreeAuditReceiptV1 {
            schema: "fn64.wm-selected-build-writer-audit.v3",
            exact_runs_per_channel: 10,
            channel_count: 8,
            completed_channel_bitmap: ALL_CHANNEL_BITS,
            build_schema: build_evidence.schema,
            build_authority_sha256: &build_evidence.authority_sha256,
            selected_binary_sha256: &build_evidence.selected_binary_sha256,
            private_build_inputs_sha256: &build_evidence.private_build_inputs_sha256,
            cargo_graph_sha256: &build_evidence.cargo_graph_sha256,
            cargo_source_sha256: &build_evidence.cargo_source_sha256,
            build_environment_sha256: &build_evidence.build_environment_sha256,
            builder_cargo_sha256: &build_evidence.builder_cargo_sha256,
            builder_rustc_sha256: &build_evidence.builder_rustc_sha256,
            cargo_config_sha256: &build_evidence.cargo_config_sha256,
            memory_guard_sha256: &build_evidence.memory_guard_sha256,
            selected_build_cargo_jobs: build_evidence.selected_build_cargo_jobs,
            build_max_rss_mib: build_evidence.build_max_rss_mib,
            build_min_free_percent: build_evidence.build_min_free_percent,
            program_identity_sha256: &build_evidence.identity.program_identity_sha256,
            normalized_rom_sha256: &build_evidence.identity.normalized_rom_sha256,
            manifest_sha256: &build_evidence.identity.manifest_sha256,
            lock_sha256: &build_evidence.identity.lock_sha256,
            root_adapter_source_sha256: &build_evidence.identity.root_adapter_source_sha256,
            shard_cargo_source_tree_sha256: &build_evidence.identity.shard_cargo_source_tree_sha256,
            emitter_source_sha256: &build_evidence.identity.emitter_source_sha256,
            runtime_source_sha256: &build_evidence.identity.runtime_source_sha256,
            prepared_tree_sha256: &build_evidence.prepared_tree_sha256,
            producer_cargo_source_sha256: &build_evidence.producer_cargo_source_sha256,
            producer_binary_sha256: &build_evidence.producer_binary_sha256,
            bundle_schema,
            bundle_authority_sha256: &bundle_authority_sha256,
            program_model_sha256: &program_model_sha256,
            writer_denominator_sha256: &denominator_sha256,
        })
        .map_err(|error| format!("serialize path-free audit receipt: {error}"))?;
        write_receipts(&output, &denominator_json, &audit_json)?;
        println!(
            "fn64.wm-selected-build-writer-audit.v3 status=complete channels=8 exact_runs=10 cargo_jobs=2 guard_mib=4096 min_free_percent=40"
        );
        Ok(())
    }

    fn write_failure_diagnostics(
        output: &Path,
        failures: &[(AuditSeries, String)],
    ) -> Result<Vec<PartialFailureDiagnosticV1>, String> {
        let diagnostics_dir = output.join("diagnostics");
        create_mode_700_directory(&diagnostics_dir)?;
        let mut diagnostics = Vec::with_capacity(failures.len());
        for (series, error) in failures {
            let bytes = error.as_bytes();
            let (retained, truncated) = bounded_failure_diagnostic(error);
            let file = format!("{}.log", series.token());
            write_private_file(&diagnostics_dir.join(&file), retained)?;
            diagnostics.push(PartialFailureDiagnosticV1 {
                channel: series.token(),
                file: format!("diagnostics/{file}"),
                total_bytes: bytes.len() as u64,
                retained_bytes: retained.len() as u64,
                truncated,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            });
        }
        Ok(diagnostics)
    }

    fn write_partial_outputs(
        output: &Path,
        build: &fn64_boot_harness::GeneratedRunnerBuildEvidenceV1,
        bundle: Option<fn64_boot_harness::VerifiedGeneratedRunnerWriterAuditBundleV1>,
        failures: &[PartialFailureDiagnosticV1],
    ) -> Result<(), String> {
        let mut completed_channel_bitmap = 0;
        let mut bundle_schema = None;
        let mut bundle_authority_sha256_reference = None;
        let mut program_model_sha256 = None;
        let mut partial_writer_denominator_sha256 = None;
        let mut successful_series_diagnostic_references = Vec::new();
        if let Some(bundle) = bundle {
            let evidence = bundle.evidence();
            completed_channel_bitmap = evidence.completed_channels;
            bundle_schema = Some(evidence.schema.to_owned());
            bundle_authority_sha256_reference = Some(evidence.authority_sha256.clone());
            program_model_sha256 = writer_bundle_program_model(evidence);
            macro_rules! retain_series {
                ($field:ident, $channel:literal) => {
                    if let Some(series) = evidence.$field.as_ref() {
                        successful_series_diagnostic_references.push(PartialSeriesReferenceV1 {
                            channel: $channel,
                            series_schema: series.schema.to_owned(),
                            series_authority_sha256_reference: series.authority_sha256.clone(),
                        });
                    }
                };
            }
            retain_series!(bootstrap, "bootstrap");
            retain_series!(cpu, "cpu");
            retain_series!(host_abi, "host_abi");
            retain_series!(pi, "pi");
            retain_series!(rdp_renderer, "rdp_renderer");
            retain_series!(rsp, "rsp");
            retain_series!(si, "si");
            retain_series!(sp, "sp");
            let model = program_model_sha256
                .as_ref()
                .ok_or_else(|| "partial writer bundle has no successful series model".to_owned())?;
            let denominator = new_open_writer_denominator(model.clone())?
                .complete_writer_audit_bundle(bundle)
                .map_err(|error| format!("consume partial writer-audit capability: {error}"))?;
            if denominator.is_complete() {
                return Err(
                    "failed writer audit unexpectedly produced a complete denominator".into(),
                );
            }
            let denominator_json = denominator
                .canonical_json_bytes()
                .map_err(|error| format!("serialize partial writer denominator: {error}"))?;
            partial_writer_denominator_sha256 =
                Some(format!("{:x}", Sha256::digest(&denominator_json)));
            write_private_file(
                &output.join(OUTPUT_PARTIAL_WRITER_DENOMINATOR),
                &denominator_json,
            )?;
        }
        let json = serde_json::to_vec(&PartialAuditReceiptV1 {
            schema: "fn64.wm-selected-build-writer-audit-partial-diagnostic.v1",
            status: "incomplete",
            exact_runs_per_successful_channel: 10,
            attempted_channel_count: ALL_AUDIT_SERIES.len() as u8,
            completed_channel_bitmap,
            build_schema: build.schema,
            build_authority_sha256: &build.authority_sha256,
            selected_binary_sha256: &build.selected_binary_sha256,
            private_build_inputs_sha256: &build.private_build_inputs_sha256,
            bundle_schema,
            bundle_authority_sha256_reference,
            program_model_sha256,
            partial_writer_denominator: partial_writer_denominator_sha256
                .as_ref()
                .map(|_| OUTPUT_PARTIAL_WRITER_DENOMINATOR),
            partial_writer_denominator_sha256,
            successful_series_diagnostic_references,
            failures,
        })
        .map_err(|error| format!("serialize partial writer-audit diagnostic: {error}"))?;
        write_private_file(&output.join(OUTPUT_PARTIAL_AUDIT_RECEIPT), &json)
    }

    fn writer_bundle_program_model(
        evidence: &fn64_boot_harness::GeneratedRunnerWriterAuditBundleEvidenceV1,
    ) -> Option<String> {
        first_program_model([
            evidence
                .bootstrap
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .cpu
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .host_abi
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .pi
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .rdp_renderer
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .rsp
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .si
                .as_ref()
                .map(|series| &series.program_model_sha256),
            evidence
                .sp
                .as_ref()
                .map(|series| &series.program_model_sha256),
        ])
    }

    pub(super) fn first_program_model<'a>(
        models: impl IntoIterator<Item = Option<&'a String>>,
    ) -> Option<String> {
        models.into_iter().flatten().next().cloned()
    }

    fn new_open_writer_denominator(
        program_model_sha256: String,
    ) -> Result<WriterChannelDenominatorV2, String> {
        WriterChannelDenominatorV2::new_open(WriterChannelDenominatorInputV2 {
            producer: "fn64-discover:run_wm_writer_audit:v1".into(),
            program_model_sha256,
            channels: WRITER_CHANNELS_V2
                .into_iter()
                .map(|channel| OpenWriterChannelInputV2 {
                    channel,
                    blockers: vec![WriterChannelBlockerV2 {
                        code: WriterChannelBlockerCodeV2::ValidatorUnavailable,
                        evidence:
                            "pending in-process selected-build writer-audit bundle consumption"
                                .into(),
                    }],
                })
                .collect(),
        })
        .map_err(|error| format!("construct fixed writer denominator: {error}"))
    }

    fn create_private_output_directory(requested: &Path) -> Result<PathBuf, String> {
        if requested.exists() || requested.is_symlink() {
            return Err("--output must name a path which does not exist".into());
        }
        let parent = requested
            .parent()
            .ok_or_else(|| "--output has no parent directory".to_owned())?
            .canonicalize()
            .map_err(|_| "--output parent must be an existing directory".to_owned())?;
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("fn64-discover manifest lost repository ancestors")
            .canonicalize()
            .map_err(|_| "cannot resolve repository root".to_owned())?;
        if parent == repository || parent.starts_with(&repository) {
            return Err("--output must be outside the repository".into());
        }
        create_mode_700_directory(requested)?;
        Ok(requested.to_owned())
    }

    fn write_receipts(output: &Path, denominator: &[u8], audit: &[u8]) -> Result<(), String> {
        let writer_temp = output.join("writers.tmp");
        let audit_temp = output.join("writer-audit.tmp");
        write_private_file(&writer_temp, denominator)?;
        write_private_file(&audit_temp, audit)?;
        fs::rename(&writer_temp, output.join(OUTPUT_WRITER_DENOMINATOR))
            .map_err(|_| "cannot publish writer denominator receipt".to_owned())?;
        fs::rename(&audit_temp, output.join(OUTPUT_AUDIT_RECEIPT))
            .map_err(|_| "cannot publish writer audit receipt".to_owned())?;
        File::open(output)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "cannot sync private output directory".to_owned())?;
        Ok(())
    }

    fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .map_err(|_| "cannot create private receipt".to_owned())?;
        set_mode(path, 0o600)?;
        file.write_all(bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| "cannot persist private receipt".to_owned())
    }

    #[cfg(unix)]
    fn create_mode_700_directory(path: &Path) -> Result<(), String> {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| "cannot create private output directory".to_owned())?;
        set_mode(path, 0o700)
    }

    #[cfg(not(unix))]
    fn create_mode_700_directory(_path: &Path) -> Result<(), String> {
        Err("private writer-audit receipt creation requires Unix permissions".into())
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|_| "cannot set private filesystem permissions".to_owned())
    }

    #[cfg(not(unix))]
    fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
        Err("private writer-audit receipt creation requires Unix permissions".into())
    }
}

#[cfg(feature = "writer-runtime-authority")]
fn main() {
    let result = parse_args(std::env::args().skip(1)).and_then(production::run);
    if let Err(error) = result {
        eprintln!("run_wm_writer_audit: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "writer-runtime-authority"))]
fn main() {
    eprintln!("run_wm_writer_audit requires fn64-discover feature writer-runtime-authority");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeSession {
        calls: Vec<AuditSeries>,
        completed: Vec<AuditSeries>,
        fail_at: Vec<AuditSeries>,
    }

    impl WriterAuditSession for FakeSession {
        type Bundle = Vec<AuditSeries>;

        fn run_series(&mut self, series: AuditSeries) -> Result<(), String> {
            self.calls.push(series);
            if self.fail_at.contains(&series) {
                return Err("synthetic series failure".into());
            }
            self.completed.push(series);
            Ok(())
        }

        fn seal(self) -> Result<Self::Bundle, String> {
            Ok(self.completed)
        }
    }

    #[test]
    fn fake_session_runs_all_eight_once_before_seal() {
        let run = run_all_series_with_progress(FakeSession::default(), |_, _| {}).unwrap();
        assert_eq!(run.bundle.unwrap(), ALL_AUDIT_SERIES);
        assert!(run.failures.is_empty());
    }

    #[test]
    fn progress_wraps_each_exact_ten_series_without_entering_evidence() {
        let mut progress = Vec::new();
        let run = run_all_series_with_progress(FakeSession::default(), |series, state| {
            progress.push((series, state));
        })
        .unwrap();

        assert_eq!(run.bundle.unwrap(), ALL_AUDIT_SERIES);
        assert!(run.failures.is_empty());
        assert_eq!(progress.len(), ALL_AUDIT_SERIES.len() * 2);
        for (index, series) in ALL_AUDIT_SERIES.into_iter().enumerate() {
            assert_eq!(
                &progress[index * 2..index * 2 + 2],
                &[
                    (series, SeriesProgress::Started),
                    (series, SeriesProgress::Completed),
                ]
            );
            assert!(!series.token().contains('/'));
        }
    }

    #[test]
    fn failed_series_do_not_discard_later_independent_series() {
        let mut progress = Vec::new();
        let run = run_all_series_with_progress(
            FakeSession {
                fail_at: vec![AuditSeries::Pi, AuditSeries::Rsp],
                ..FakeSession::default()
            },
            |series, state| progress.push((series, state)),
        )
        .unwrap();
        assert_eq!(
            run.bundle.unwrap(),
            vec![
                AuditSeries::Bootstrap,
                AuditSeries::Cpu,
                AuditSeries::HostAbi,
                AuditSeries::RdpRenderer,
                AuditSeries::Si,
                AuditSeries::Sp,
            ]
        );
        assert_eq!(
            run.failures
                .iter()
                .map(|(series, _)| *series)
                .collect::<Vec<_>>(),
            vec![AuditSeries::Pi, AuditSeries::Rsp]
        );
        assert_eq!(
            progress.last(),
            Some(&(AuditSeries::Sp, SeriesProgress::Completed))
        );
        assert!(progress.contains(&(AuditSeries::Pi, SeriesProgress::Failed)));
        assert!(progress.contains(&(AuditSeries::Rsp, SeriesProgress::Failed)));
        assert!(!progress.contains(&(AuditSeries::Pi, SeriesProgress::Completed)));
    }

    #[test]
    fn all_failed_series_are_inspectable_without_minting_a_bundle() {
        let run = run_all_series_with_progress(
            FakeSession {
                fail_at: ALL_AUDIT_SERIES.to_vec(),
                ..FakeSession::default()
            },
            |_, _| {},
        )
        .unwrap();
        assert!(run.bundle.is_none());
        assert_eq!(run.failures.len(), ALL_AUDIT_SERIES.len());
    }

    #[test]
    fn failure_diagnostics_have_a_fixed_retention_bound() {
        let oversized = "x".repeat(FAILURE_DIAGNOSTIC_LIMIT + 17);
        let (retained, truncated) = bounded_failure_diagnostic(&oversized);
        assert_eq!(retained.len(), FAILURE_DIAGNOSTIC_LIMIT);
        assert!(truncated);

        let (retained, truncated) = bounded_failure_diagnostic("short");
        assert_eq!(retained, b"short");
        assert!(!truncated);
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn partial_model_falls_back_to_a_non_bootstrap_success() {
        let cpu_model = "1".repeat(64);
        let rsp_model = "2".repeat(64);
        assert_eq!(
            production::first_program_model([None, Some(&cpu_model), Some(&rsp_model)]),
            Some(cpu_model)
        );
        assert_eq!(production::first_program_model([None, None]), None);
    }

    #[test]
    fn parser_requires_exact_private_input_shapes_without_echoing_paths() {
        let parsed = parse_args(
            [
                "--rom",
                "/private/rom.z64",
                "--boot-context",
                "/private/boot.json",
                "--image-group",
                "FN64_EXECUTABLE_IMAGE_MAIN",
                "/private/a.bin",
                "/private/b.bin",
                "/private/c.bin",
                "--output",
                "/private/new-audit",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(parsed.image_groups.len(), 1);
        assert_eq!(parsed.image_groups[0].captures.len(), 3);
        assert_eq!(parsed.max_build_seconds, DEFAULT_MAX_BUILD_SECONDS);

        let error = parse_args(
            [
                "--rom",
                "relative-rom",
                "--boot-context",
                "/private/boot.json",
                "--image-group",
                "FN64_EXECUTABLE_IMAGE_MAIN",
                "/private/a.bin",
                "/private/b.bin",
                "/private/c.bin",
                "--output",
                "/private/new-audit",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap_err();
        assert!(!error.contains("relative-rom"));
    }
}
