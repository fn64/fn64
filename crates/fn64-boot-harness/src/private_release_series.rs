//! Trusted-process orchestration for one fixed-cycle release series.
//!
//! A retained report series can prove semantic equality and distinct event
//! identities, but it cannot prove who created those identities. During an
//! observed invocation this module owns the missing orchestration boundary:
//! one process creates a random series nonce, launches exactly ten sequential
//! child processes, and verifies each durable report/journal pair before
//! launching the next child. Its retained receipt is an integrity record, not
//! a signature or later proof that this process performed those launches.

#[cfg(test)]
use crate::release_program_build_receipt::ReleaseProgramBuildReceipt;
use crate::release_program_build_receipt::{
    load_release_program_build_receipt, ReleaseProgramBuildLane, ReleaseProgramFileIdentity,
    VerifiedReleaseProgramBuildReceipt, RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA,
};
use crate::{
    parse_unsupported_journal, verify_release_evidence_series, verify_release_report_journal,
    ArtifactKind, ExecutionDestinationSource, ParsedUnsupportedJournal, ReleaseGateReport,
    ReleaseRomClass, ReleaseRomEvidence, ReleaseTvStandard, RspRdpObservationKindEvidence,
    LIVE_MINIMUM_CLOSURE_PATHS, RELEASE_GATE_CYCLE_ENV, RELEASE_REPORT_ENV, RELEASE_ROM_CLASS_ENV,
    RELEASE_RUN_EVENT_SHA256_ENV,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

pub const PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA: &str = "fn64.private-release-run-contract.v3";
pub const PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA: &str = "fn64.private-release-series-receipt.v1";
pub const PRIVATE_RELEASE_SERIES_COUNT: usize = 10;
pub const RELEASE_MICROCODE_TEXT_PATH_ENV: &str = "FN64_RELEASE_MICROCODE_TEXT_PATH";
pub const RELEASE_MICROCODE_DATA_PATH_ENV: &str = "FN64_RELEASE_MICROCODE_DATA_PATH";
pub const REPOSITORY_SYNTHETIC_RELEASE_SCENARIO: &str =
    "synthetic-runtime-device-render-fixed-cycle-v1";
pub const REPOSITORY_SYNTHETIC_RELEASE_CYCLE: u64 = 1_562_500;
pub const REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES: &[u8] =
    b"repository synthetic runner manifest v1\n";
pub const REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES: &[u8] =
    b"repository synthetic readiness v1\n";
pub const REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES: &[u8] =
    b"fn64 synthetic non-game release input v1";

const RELEASE_REPORT_SCHEMA: &str = "fn64.release-gate.v21";
const CONTRACT_DIGEST_DOMAIN: &[u8] = b"fn64.private-release-run-contract-digest.v3\0";
const RUN_EVENT_DOMAIN: &[u8] = b"fn64.private-release-run-event.v1\0";
const RECEIPT_DIGEST_DOMAIN: &[u8] = b"fn64.private-release-series-receipt-digest.v1\0";
const RECEIPT_FILE: &str = "receipt.json";
const SYSTEM_PYTHON3: &str = "/usr/bin/python3";
const PYTHON_EMBEDDED_SCRIPT_BOOTSTRAP: &str = "import sys\nscript_path = sys.argv.pop(1)\nsource = sys.stdin.buffer.read()\nexec(compile(source, script_path, 'exec'), {'__name__': '__main__', '__file__': script_path})";
const PRIVATE_INPUT_ADMISSION_SCRIPT: &[u8] =
    include_bytes!("../../../tools/private_input_admission.py");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateFileIdentity {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateArtifactIdentity {
    pub role: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub provenance: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateEnvironmentEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateChildCommand {
    pub executable: PrivateFileIdentity,
    pub working_directory: String,
    pub argv: Vec<String>,
    pub environment: Vec<PrivateEnvironmentEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateReleaseRunContract {
    pub schema: String,
    pub admission_manifest: PrivateFileIdentity,
    pub readiness_report: PrivateFileIdentity,
    /// Exact private receipt which recomputes the linked program identity from
    /// its lane inputs and binds it to the child executable image.
    pub program_build_receipt: Option<PrivateFileIdentity>,
    pub purpose: String,
    pub rom_class: ReleaseRomClass,
    pub report_scenario: String,
    pub guest_cycle: u64,
    pub repeat_count: usize,
    pub input: PrivateArtifactIdentity,
    pub admitted_artifacts: Vec<PrivateArtifactIdentity>,
    pub expected_execution_source: ExecutionDestinationSource,
    pub child: PrivateChildCommand,
    pub contract_sha256: String,
}

/// A release-run contract whose construction passed the authority appropriate
/// to its purpose. The inner contract is deliberately inaccessible so callers
/// cannot deserialize a self-hashed JSON object and present it to the runner as
/// an admitted production contract.
#[derive(Debug)]
pub struct VerifiedPrivateReleaseRunContract {
    contract: PrivateReleaseRunContract,
}

impl VerifiedPrivateReleaseRunContract {
    fn contract(&self) -> &PrivateReleaseRunContract {
        &self.contract
    }
}

/// One exact retained series whose contract, receipt, runner, reports,
/// journals, and admitted inputs passed a fresh verification together.
///
/// The fields stay private so a deserialized contract or self-hashed receipt
/// cannot be upgraded into matrix authority without re-reading the retained
/// files and independently reconstructing the admitted ROM evidence.
#[derive(Debug)]
pub struct VerifiedPrivateReleaseSeries {
    contract: PrivateReleaseRunContract,
    output_directory: PathBuf,
    receipt: PrivateReleaseSeriesReceipt,
    runner_executable: PathBuf,
}

#[derive(Debug)]
pub(crate) struct RevalidatedPrivateReleaseSeries {
    pub(crate) contract: PrivateReleaseRunContract,
    pub(crate) receipt: PrivateReleaseSeriesReceipt,
}

impl VerifiedPrivateReleaseSeries {
    /// Re-read the complete retained series immediately before matrix use.
    /// Returning owned identity snapshots prevents later authority derivation
    /// from consulting ambient paths independently of this verification.
    pub(crate) fn revalidate_for_release_matrix(
        &self,
    ) -> Result<RevalidatedPrivateReleaseSeries, PrivateReleaseSeriesError> {
        verify_private_release_series_inner(
            &self.contract,
            &self.output_directory,
            &self.receipt,
            &self.runner_executable,
        )?;
        Ok(RevalidatedPrivateReleaseSeries {
            contract: self.contract.clone(),
            receipt: self.receipt.clone(),
        })
    }
}

impl PrivateReleaseRunContract {
    pub fn recompute_contract_sha256(&self) -> Result<String, PrivateReleaseSeriesError> {
        let mut wire = Vec::new();
        wire.extend_from_slice(CONTRACT_DIGEST_DOMAIN);
        push_bytes(&mut wire, self.schema.as_bytes());
        encode_file_identity(&mut wire, &self.admission_manifest)?;
        encode_file_identity(&mut wire, &self.readiness_report)?;
        match &self.program_build_receipt {
            Some(receipt) => {
                wire.push(1);
                encode_file_identity(&mut wire, receipt)?;
            }
            None => wire.push(0),
        }
        push_bytes(&mut wire, self.purpose.as_bytes());
        push_bytes(&mut wire, self.rom_class.wire_name().as_bytes());
        push_bytes(&mut wire, self.report_scenario.as_bytes());
        push_u64(&mut wire, self.guest_cycle);
        push_u64(
            &mut wire,
            u64::try_from(self.repeat_count)
                .map_err(|_| error("contract repeat count exceeds the canonical wire"))?,
        );
        encode_artifact_identity(&mut wire, &self.input)?;
        push_u64(
            &mut wire,
            u64::try_from(self.admitted_artifacts.len())
                .map_err(|_| error("contract artifact count exceeds the canonical wire"))?,
        );
        for artifact in &self.admitted_artifacts {
            encode_artifact_identity(&mut wire, artifact)?;
        }
        encode_execution_source(&mut wire, &self.expected_execution_source)?;
        encode_file_identity(&mut wire, &self.child.executable)?;
        push_bytes(&mut wire, self.child.working_directory.as_bytes());
        push_u64(
            &mut wire,
            u64::try_from(self.child.argv.len())
                .map_err(|_| error("contract argv count exceeds the canonical wire"))?,
        );
        for argument in &self.child.argv {
            push_bytes(&mut wire, argument.as_bytes());
        }
        push_u64(
            &mut wire,
            u64::try_from(self.child.environment.len())
                .map_err(|_| error("contract environment count exceeds the canonical wire"))?,
        );
        for entry in &self.child.environment {
            push_bytes(&mut wire, entry.name.as_bytes());
            push_bytes(&mut wire, entry.value.as_bytes());
        }
        Ok(sha256_hex(&wire))
    }

    pub fn verify_integrity(&self) -> Result<(), PrivateReleaseSeriesError> {
        self.verify_shape()?;
        require_sha256(&self.contract_sha256, "contract_sha256")?;
        let recomputed = self.recompute_contract_sha256()?;
        if recomputed != self.contract_sha256 {
            return Err(error(format!(
                "private release contract digest mismatch: stored {}, recomputed {recomputed}",
                self.contract_sha256
            )));
        }
        Ok(())
    }

    fn verify_shape(&self) -> Result<(), PrivateReleaseSeriesError> {
        if self.schema != PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA {
            return Err(error(format!(
                "unsupported private release contract schema {:?}",
                self.schema
            )));
        }
        if self.repeat_count != PRIVATE_RELEASE_SERIES_COUNT {
            return Err(error(format!(
                "private release contract repeat_count is {}; exactly {} fresh processes are required",
                self.repeat_count, PRIVATE_RELEASE_SERIES_COUNT
            )));
        }
        validate_scenario(&self.report_scenario)?;
        match self.purpose.as_str() {
            "full_rom" | "combined" => {
                if self.input.role != "rom" {
                    return Err(error(format!(
                        "private {} contract input role must be rom, got {:?}",
                        self.purpose, self.input.role
                    )));
                }
                if matches!(
                    self.expected_execution_source,
                    ExecutionDestinationSource::NoProgram
                ) {
                    return Err(error(format!(
                        "private {} contract requires an authoritative executable source",
                        self.purpose
                    )));
                }
                if self.program_build_receipt.is_none() {
                    return Err(error(format!(
                        "private {} contract requires a {RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA}",
                        self.purpose
                    )));
                }
                let expected_provenance = match self.rom_class {
                    ReleaseRomClass::RetailCartridge => {
                        "user_owned_retail_cartridge_dump"
                    }
                    ReleaseRomClass::PublicHomebrew => {
                        "publicly_distributed_homebrew_rom"
                    }
                    ReleaseRomClass::Unclassified => {
                        return Err(error(format!(
                            "private {} contract requires an admitted retail_cartridge or public_homebrew ROM class",
                            self.purpose
                        )))
                    }
                };
                if self.input.provenance != expected_provenance {
                    return Err(error(format!(
                        "private {} contract ROM provenance {:?} does not match class {}",
                        self.purpose,
                        self.input.provenance,
                        self.rom_class.wire_name()
                    )));
                }
            }
            "synthetic_mechanism" => {
                if self.input.role != "synthetic_input" {
                    return Err(error(format!(
                        "synthetic mechanism contract input role must be synthetic_input, got {:?}",
                        self.input.role
                    )));
                }
                if self.program_build_receipt.is_some() {
                    return Err(error(
                        "synthetic mechanism contract cannot bind a production program-build receipt",
                    ));
                }
                if self.rom_class != ReleaseRomClass::Unclassified {
                    return Err(error(
                        "synthetic mechanism contract ROM class must be unclassified",
                    ));
                }
            }
            purpose => {
                return Err(error(format!(
                    "private release contract purpose {purpose:?} is unsupported"
                )))
            }
        }
        validate_file_identity_shape(&self.admission_manifest, "admission_manifest")?;
        validate_file_identity_shape(&self.readiness_report, "readiness_report")?;
        if let Some(receipt) = &self.program_build_receipt {
            validate_file_identity_shape(receipt, "program_build_receipt")?;
        }
        validate_artifact_identity_shape(&self.input, "input")?;
        validate_execution_source(&self.expected_execution_source)?;
        validate_file_identity_shape(&self.child.executable, "child.executable")?;

        let mut previous_role: Option<&str> = None;
        for artifact in &self.admitted_artifacts {
            validate_artifact_identity_shape(artifact, "admitted_artifacts[]")?;
            if previous_role.is_some_and(|previous| previous >= artifact.role.as_str()) {
                return Err(error(
                    "admitted_artifacts must be strictly sorted by unique role",
                ));
            }
            if artifact.role == self.input.role {
                return Err(error(format!(
                    "admitted_artifacts repeats the separately bound input role {:?}",
                    artifact.role
                )));
            }
            previous_role = Some(&artifact.role);
        }
        if matches!(self.purpose.as_str(), "full_rom" | "combined") {
            let roles = self
                .admitted_artifacts
                .iter()
                .map(|artifact| artifact.role.as_str())
                .collect::<BTreeSet<_>>();
            if roles != BTreeSet::from(["microcode_data", "microcode_text", "recompiled"]) {
                return Err(error(format!(
                    "private {} contract requires exact admitted roles microcode_data, microcode_text, and recompiled",
                    self.purpose
                )));
            }
            let text = required_artifact(self, "microcode_text")?;
            if text.bytes != fn64_runtime::RSP_MEMORY_BANK_SIZE as u64 {
                return Err(error(format!(
                    "microcode_text bytes are {}; exact RSP IMEM image size {} is required",
                    text.bytes,
                    fn64_runtime::RSP_MEMORY_BANK_SIZE
                )));
            }
            let data = required_artifact(self, "microcode_data")?;
            if data.bytes > u64::from(u32::MAX) {
                return Err(error(
                    "microcode_data bytes exceed the task-header u32 size field",
                ));
            }
        }

        validate_absolute_no_symlink_directory(
            Path::new(&self.child.working_directory),
            "child.working_directory",
        )?;
        for (index, argument) in self.child.argv.iter().enumerate() {
            if argument.contains('\0') {
                return Err(error(format!("child.argv[{index}] contains NUL")));
            }
        }
        let mut previous_name: Option<&str> = None;
        for entry in &self.child.environment {
            validate_environment_entry(entry)?;
            if previous_name.is_some_and(|previous| previous >= entry.name.as_str()) {
                return Err(error(
                    "child.environment must be strictly sorted by unique uppercase name",
                ));
            }
            previous_name = Some(&entry.name);
        }
        Ok(())
    }

    fn verify_bound_files(&self) -> Result<(), PrivateReleaseSeriesError> {
        verify_private_file_identity(&self.admission_manifest, "admission_manifest")?;
        verify_private_file_identity(&self.readiness_report, "readiness_report")?;
        if let Some(receipt) = &self.program_build_receipt {
            verify_private_file_identity(receipt, "program_build_receipt")?;
        }
        verify_private_artifact_identity(&self.input, "input")?;
        for artifact in &self.admitted_artifacts {
            verify_private_artifact_identity(artifact, "admitted_artifacts[]")?;
        }
        verify_file_identity(&self.child.executable, "child.executable", false)?;
        validate_native_executable(Path::new(&self.child.executable.path))?;
        validate_absolute_no_symlink_directory(
            Path::new(&self.child.working_directory),
            "child.working_directory",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateReleaseSeriesRun {
    pub ordinal: u64,
    pub run_event_sha256: String,
    pub report_file_sha256: String,
    pub journal_file_sha256: String,
    pub report_sha256: String,
    pub artifact_root_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical integrity record for a completed series. This self-hashed value
/// is not a signature or an operating-system process attestation.
pub struct PrivateReleaseSeriesReceipt {
    pub schema: String,
    pub contract_sha256: String,
    pub runner_executable_sha256: String,
    pub child_executable_sha256: String,
    pub series_nonce: String,
    pub report_scenario: String,
    pub input_sha256: String,
    pub guest_cycle: u64,
    pub expected_execution_source: ExecutionDestinationSource,
    pub count: usize,
    pub semantic_report_sha256: String,
    pub runs: Vec<PrivateReleaseSeriesRun>,
    pub receipt_sha256: String,
}

impl PrivateReleaseSeriesReceipt {
    pub fn recompute_receipt_sha256(&self) -> Result<String, PrivateReleaseSeriesError> {
        let mut wire = Vec::new();
        wire.extend_from_slice(RECEIPT_DIGEST_DOMAIN);
        push_bytes(&mut wire, self.schema.as_bytes());
        push_hash(&mut wire, &self.contract_sha256, "contract_sha256")?;
        push_hash(
            &mut wire,
            &self.runner_executable_sha256,
            "runner_executable_sha256",
        )?;
        push_hash(
            &mut wire,
            &self.child_executable_sha256,
            "child_executable_sha256",
        )?;
        push_hash(&mut wire, &self.series_nonce, "series_nonce")?;
        push_bytes(&mut wire, self.report_scenario.as_bytes());
        push_hash(&mut wire, &self.input_sha256, "input_sha256")?;
        push_u64(&mut wire, self.guest_cycle);
        encode_execution_source(&mut wire, &self.expected_execution_source)?;
        push_u64(
            &mut wire,
            u64::try_from(self.count)
                .map_err(|_| error("receipt count exceeds the canonical wire"))?,
        );
        push_hash(
            &mut wire,
            &self.semantic_report_sha256,
            "semantic_report_sha256",
        )?;
        push_u64(
            &mut wire,
            u64::try_from(self.runs.len())
                .map_err(|_| error("receipt run count exceeds the canonical wire"))?,
        );
        for run in &self.runs {
            push_u64(&mut wire, run.ordinal);
            push_hash(&mut wire, &run.run_event_sha256, "runs[].run_event_sha256")?;
            push_hash(
                &mut wire,
                &run.report_file_sha256,
                "runs[].report_file_sha256",
            )?;
            push_hash(
                &mut wire,
                &run.journal_file_sha256,
                "runs[].journal_file_sha256",
            )?;
            push_hash(&mut wire, &run.report_sha256, "runs[].report_sha256")?;
            push_hash(
                &mut wire,
                &run.artifact_root_sha256,
                "runs[].artifact_root_sha256",
            )?;
        }
        Ok(sha256_hex(&wire))
    }

    pub fn verify_integrity(&self) -> Result<(), PrivateReleaseSeriesError> {
        if self.schema != PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA {
            return Err(error(format!(
                "unsupported private release series receipt schema {:?}",
                self.schema
            )));
        }
        if self.count != PRIVATE_RELEASE_SERIES_COUNT
            || self.runs.len() != PRIVATE_RELEASE_SERIES_COUNT
        {
            return Err(error(format!(
                "private release receipt retains count={} and {} runs; exactly {} are required",
                self.count,
                self.runs.len(),
                PRIVATE_RELEASE_SERIES_COUNT
            )));
        }
        validate_scenario(&self.report_scenario)?;
        validate_execution_source(&self.expected_execution_source)?;
        for (field, value) in [
            ("contract_sha256", &self.contract_sha256),
            ("runner_executable_sha256", &self.runner_executable_sha256),
            ("child_executable_sha256", &self.child_executable_sha256),
            ("series_nonce", &self.series_nonce),
            ("input_sha256", &self.input_sha256),
            ("semantic_report_sha256", &self.semantic_report_sha256),
            ("receipt_sha256", &self.receipt_sha256),
        ] {
            require_sha256(value, field)?;
        }
        let mut events = BTreeSet::new();
        for (index, run) in self.runs.iter().enumerate() {
            let expected = u64::try_from(index + 1).expect("ten runs fit u64");
            if run.ordinal != expected {
                return Err(error(format!(
                    "private release receipt run at index {index} has ordinal {}; expected {expected}",
                    run.ordinal
                )));
            }
            for (field, value) in [
                ("runs[].run_event_sha256", &run.run_event_sha256),
                ("runs[].report_file_sha256", &run.report_file_sha256),
                ("runs[].journal_file_sha256", &run.journal_file_sha256),
                ("runs[].report_sha256", &run.report_sha256),
                ("runs[].artifact_root_sha256", &run.artifact_root_sha256),
            ] {
                require_sha256(value, field)?;
            }
            if !events.insert(&run.run_event_sha256) {
                return Err(error(format!(
                    "private release receipt repeats run-event identity {}",
                    run.run_event_sha256
                )));
            }
            if run.report_sha256 != self.semantic_report_sha256 {
                return Err(error(format!(
                    "receipt run {} report SHA {} differs from series SHA {}",
                    run.ordinal, run.report_sha256, self.semantic_report_sha256
                )));
            }
        }
        let recomputed = self.recompute_receipt_sha256()?;
        if recomputed != self.receipt_sha256 {
            return Err(error(format!(
                "private release receipt digest mismatch: stored {}, recomputed {recomputed}",
                self.receipt_sha256
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateReleaseSeriesError(String);

impl fmt::Display for PrivateReleaseSeriesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PrivateReleaseSeriesError {}

struct StagedAdmissionContract(PathBuf);

impl Drop for StagedAdmissionContract {
    fn drop(&mut self) {
        make_removable(&self.0);
        let _ = fs::remove_file(&self.0);
    }
}

struct StagedChildExecutable(PathBuf);

impl Drop for StagedChildExecutable {
    fn drop(&mut self) {
        make_removable(&self.0);
        let _ = fs::remove_file(&self.0);
    }
}

struct StagedPrivateArtifact(PathBuf);

impl Drop for StagedPrivateArtifact {
    fn drop(&mut self) {
        make_removable(&self.0);
        let _ = fs::remove_file(&self.0);
    }
}

struct StagedMicrocodePair {
    text: StagedPrivateArtifact,
    data: StagedPrivateArtifact,
    text_identity: PrivateFileIdentity,
    data_identity: PrivateFileIdentity,
}

impl StagedMicrocodePair {
    fn verify(&self) -> Result<(), PrivateReleaseSeriesError> {
        verify_file_identity(&self.text_identity, "staged microcode text", false)?;
        verify_file_identity(&self.data_identity, "staged microcode data", false)
    }
}

#[cfg(windows)]
fn make_removable(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
    }
}

#[cfg(not(windows))]
fn make_removable(_path: &Path) {}

fn open_private_stage(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn seal_private_stage(path: &Path, executable: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if executable { 0o500 } else { 0o400 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)
    }
}

fn stage_child_executable(
    identity: &PrivateFileIdentity,
) -> Result<StagedChildExecutable, PrivateReleaseSeriesError> {
    let source = Path::new(&identity.path);
    let parent = source
        .parent()
        .ok_or_else(|| error("child executable has no parent directory"))?;
    for _ in 0..32 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|source| error(format!("obtain child-stage nonce: {source}")))?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default();
        let path = parent.join(format!(".fn64-release-child-{}{}", hex(&random), extension));
        match open_private_stage(&path) {
            Ok(mut destination) => {
                let staged = StagedChildExecutable(path.clone());
                let mut source_file = File::open(source).map_err(|source| {
                    error(format!(
                        "open child executable {} for exact staging: {source}",
                        identity.path
                    ))
                })?;
                std::io::copy(&mut source_file, &mut destination)
                    .and_then(|_| destination.flush())
                    .and_then(|()| destination.sync_all())
                    .map_err(|source| {
                        error(format!(
                            "persist exact staged child executable {}: {source}",
                            path.display()
                        ))
                    })?;
                seal_private_stage(&path, true).map_err(|source| {
                    error(format!(
                        "seal staged child executable owner-only {}: {source}",
                        path.display()
                    ))
                })?;
                let staged_identity = PrivateFileIdentity {
                    path: path
                        .to_str()
                        .ok_or_else(|| error("staged child executable path is not UTF-8"))?
                        .to_owned(),
                    bytes: identity.bytes,
                    sha256: identity.sha256.clone(),
                };
                verify_file_identity(&staged_identity, "staged child executable", false)?;
                validate_native_executable(&path)?;
                return Ok(staged);
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(error(format!(
                    "create exact child-executable stage beside {}: {source}",
                    identity.path
                )))
            }
        }
    }
    Err(error(format!(
        "could not allocate an exact staged child executable beside {}",
        identity.path
    )))
}

fn stage_private_artifact(
    identity: &PrivateArtifactIdentity,
) -> Result<(StagedPrivateArtifact, PrivateFileIdentity), PrivateReleaseSeriesError> {
    let source = Path::new(&identity.path);
    let parent = source.parent().ok_or_else(|| {
        error(format!(
            "private artifact {:?} has no parent directory",
            identity.role
        ))
    })?;
    for _ in 0..32 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|source| error(format!("obtain artifact-stage nonce: {source}")))?;
        let path = parent.join(format!(".fn64-release-{}-{}", identity.role, hex(&random)));
        match open_private_stage(&path) {
            Ok(mut destination) => {
                let staged = StagedPrivateArtifact(path.clone());
                let mut source_file = File::open(source).map_err(|source| {
                    error(format!(
                        "open private artifact {:?} for exact staging: {source}",
                        identity.role
                    ))
                })?;
                std::io::copy(&mut source_file, &mut destination)
                    .and_then(|_| destination.flush())
                    .and_then(|()| destination.sync_all())
                    .map_err(|source| {
                        error(format!(
                            "persist exact staged private artifact {:?}: {source}",
                            identity.role
                        ))
                    })?;
                seal_private_stage(&path, false).map_err(|source| {
                    error(format!(
                        "seal staged private artifact {:?} owner-only: {source}",
                        identity.role
                    ))
                })?;
                let staged_identity = PrivateFileIdentity {
                    path: path
                        .to_str()
                        .ok_or_else(|| error("staged private artifact path is not UTF-8"))?
                        .to_owned(),
                    bytes: identity.bytes,
                    sha256: identity.sha256.clone(),
                };
                verify_file_identity(&staged_identity, "staged private artifact", false)?;
                return Ok((staged, staged_identity));
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(error(format!(
                    "create exact private-artifact stage beside {:?}: {source}",
                    identity.role
                )))
            }
        }
    }
    Err(error(format!(
        "could not allocate an exact staged private artifact for {:?}",
        identity.role
    )))
}

fn stage_microcode_pair(
    contract: &PrivateReleaseRunContract,
) -> Result<Option<StagedMicrocodePair>, PrivateReleaseSeriesError> {
    if contract.purpose == "synthetic_mechanism" {
        return Ok(None);
    }
    let (text, text_identity) =
        stage_private_artifact(required_artifact(contract, "microcode_text")?)?;
    let (data, data_identity) =
        stage_private_artifact(required_artifact(contract, "microcode_data")?)?;
    let pair = StagedMicrocodePair {
        text,
        data,
        text_identity,
        data_identity,
    };
    pair.verify()?;
    Ok(Some(pair))
}

fn stage_contract_for_admission(
    source_path: &Path,
    bytes: &[u8],
) -> Result<StagedAdmissionContract, PrivateReleaseSeriesError> {
    let parent = source_path
        .parent()
        .ok_or_else(|| error("private run contract has no parent directory"))?;
    for _ in 0..32 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|source| error(format!("obtain admission-stage nonce: {source}")))?;
        let path = parent.join(format!(
            ".fn64-private-contract-admission-{}.json",
            hex(&random)
        ));
        match open_private_stage(&path) {
            Ok(mut file) => {
                let staged = StagedAdmissionContract(path.clone());
                file.write_all(bytes)
                    .and_then(|()| file.flush())
                    .and_then(|()| file.sync_all())
                    .map_err(|source| {
                        error(format!(
                            "persist admission-stage contract {}: {source}",
                            path.display()
                        ))
                    })?;
                seal_private_stage(&path, false).map_err(|source| {
                    error(format!(
                        "seal admission-stage contract {} owner-only: {source}",
                        path.display()
                    ))
                })?;
                return Ok(staged);
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(error(format!(
                    "create admission-stage contract in {}: {source}",
                    parent.display()
                )))
            }
        }
    }
    Err(error(format!(
        "could not allocate a unique admission-stage contract in {}",
        parent.display()
    )))
}

fn verify_with_repository_admission(
    source_path: &Path,
    contract_bytes: &[u8],
) -> Result<(), PrivateReleaseSeriesError> {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|source| error(format!("resolve fn64 repository root: {source}")))?;
    let script_path = repository_root.join("tools/private_input_admission.py");
    validate_regular_no_symlink_file(&script_path, "private input admission script")?;
    let runtime_script = fs::read(&script_path).map_err(|source| {
        error(format!(
            "read private input admission script {}: {source}",
            script_path.display()
        ))
    })?;
    if runtime_script != PRIVATE_INPUT_ADMISSION_SCRIPT {
        return Err(error(format!(
            "private input admission script {} differs from the bytes embedded in this runner; rebuild the runner from the current repository before verifying a production contract",
            script_path.display()
        )));
    }

    let system_python = Path::new(SYSTEM_PYTHON3)
        .canonicalize()
        .map_err(|source| error(format!("resolve pinned system python3: {source}")))?;
    validate_regular_no_symlink_file(&system_python, "resolved system python3")?;
    validate_native_executable(&system_python)?;
    // Python and Rust must authorize the same immutable document. Verifying a
    // create-new staged copy closes the pathname swap between the policy
    // process and Rust deserialization; the source path remains provenance,
    // while these exact bytes are the authority consumed below.
    let staged = stage_contract_for_admission(source_path, contract_bytes)?;
    let mut child = Command::new(&system_python)
        .args(["-I", "-B"])
        .args(["-c", PYTHON_EMBEDDED_SCRIPT_BOOTSTRAP])
        .arg(&script_path)
        .arg("--verify-private-run-contract")
        .arg(&staged.0)
        .current_dir(&repository_root)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| {
            error(format!(
                "spawn embedded private input admission verifier with {}: {source}",
                system_python.display()
            ))
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| error("embedded admission verifier stdin was not piped"))?
        .write_all(PRIVATE_INPUT_ADMISSION_SCRIPT)
        .map_err(|source| {
            error(format!(
                "write embedded admission policy to python3: {source}"
            ))
        })?;
    let output = child.wait_with_output().map_err(|source| {
        error(format!(
            "wait for embedded private input admission verifier: {source}"
        ))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(error(format!(
            "repository private input admission verifier rejected staged bytes from {} with {}: {}",
            source_path.display(),
            output.status,
            stderr.trim()
        )));
    }
    Ok(())
}

fn same_program_file_identity(
    build: &ReleaseProgramFileIdentity,
    contract_path: &str,
    contract_bytes: u64,
    contract_sha256: &str,
) -> bool {
    build.path == contract_path && build.bytes == contract_bytes && build.sha256 == contract_sha256
}

fn required_artifact<'a>(
    contract: &'a PrivateReleaseRunContract,
    role: &str,
) -> Result<&'a PrivateArtifactIdentity, PrivateReleaseSeriesError> {
    contract
        .admitted_artifacts
        .iter()
        .find(|artifact| artifact.role == role)
        .ok_or_else(|| {
            error(format!(
                "private release contract omits required artifact {role:?}"
            ))
        })
}

fn verify_release_program_build_receipt_binding(
    contract: &PrivateReleaseRunContract,
) -> Result<Option<VerifiedReleaseProgramBuildReceipt>, PrivateReleaseSeriesError> {
    match contract.purpose.as_str() {
        "full_rom" | "combined" => {
            let identity = contract.program_build_receipt.as_ref().ok_or_else(|| {
                error(format!(
                    "private {} execution requires a {RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA}",
                    contract.purpose
                ))
            })?;
            verify_private_file_identity(identity, "program_build_receipt")?;
            let verified =
                load_release_program_build_receipt(&identity.path).map_err(|source| {
                    error(format!(
                        "verify bound {RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA}: {source}"
                    ))
                })?;
            verify_private_file_identity(identity, "program_build_receipt")?;
            if !same_program_file_identity(
                &verified.receipt.child_executable,
                &contract.child.executable.path,
                contract.child.executable.bytes,
                &contract.child.executable.sha256,
            ) {
                return Err(error(
                    "program-build receipt child executable does not match the private run contract",
                ));
            }
            if verified.receipt.expected_execution_source != contract.expected_execution_source
                || verified.recomputed_execution_source != contract.expected_execution_source
            {
                return Err(error(
                    "program-build receipt execution source does not match the private run contract",
                ));
            }
            let recompiled = required_artifact(contract, "recompiled")?;
            let matching_inputs = match &verified.receipt.lane {
                ReleaseProgramBuildLane::NativeArchives { archives } => archives
                    .iter()
                    .filter(|archive| {
                        same_program_file_identity(
                            &archive.file,
                            &recompiled.path,
                            recompiled.bytes,
                            &recompiled.sha256,
                        )
                    })
                    .count(),
                ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } => {
                    usize::from(same_program_file_identity(
                        identity_wire,
                        &recompiled.path,
                        recompiled.bytes,
                        &recompiled.sha256,
                    ))
                }
                ReleaseProgramBuildLane::TypedBlock { pack, .. } => {
                    usize::from(same_program_file_identity(
                        pack,
                        &recompiled.path,
                        recompiled.bytes,
                        &recompiled.sha256,
                    ))
                }
            };
            if matching_inputs != 1 {
                return Err(error(format!(
                    "program-build receipt binds {matching_inputs} exact lane inputs matching the admitted recompiled artifact; exactly one is required"
                )));
            }
            Ok(Some(verified))
        }
        "synthetic_mechanism" => {
            if contract.program_build_receipt.is_some() {
                return Err(error(
                    "synthetic mechanism contract cannot bind a production program-build receipt",
                ));
            }
            Ok(None)
        }
        _ => Err(error("private release contract purpose was not validated")),
    }
}

pub fn load_private_release_run_contract(
    path: impl AsRef<Path>,
) -> Result<VerifiedPrivateReleaseRunContract, PrivateReleaseSeriesError> {
    let path = path.as_ref();
    validate_private_existing_path(path, "private run contract")?;
    let bytes = fs::read(path).map_err(|source| {
        error(format!(
            "read private run contract {}: {source}",
            path.display()
        ))
    })?;
    verify_with_repository_admission(path, &bytes)?;
    let contract: PrivateReleaseRunContract = serde_json::from_slice(&bytes).map_err(|source| {
        error(format!(
            "parse private run contract {}: {source}",
            path.display()
        ))
    })?;
    contract.verify_integrity()?;
    verify_release_program_build_receipt_binding(&contract)?;
    Ok(VerifiedPrivateReleaseRunContract { contract })
}

fn identity_matches_bytes(identity: &PrivateFileIdentity, bytes: &[u8]) -> bool {
    identity.bytes == u64::try_from(bytes.len()).expect("fixture length fits u64")
        && identity.sha256 == sha256_hex(bytes)
}

/// Admit only fn64's fixed, non-game synthetic mechanism fixture without the
/// content-bearing Python path. Arbitrary caller-labelled synthetic inputs do
/// not create runner authority.
pub fn verify_repository_synthetic_private_release_run_contract(
    contract: PrivateReleaseRunContract,
) -> Result<VerifiedPrivateReleaseRunContract, PrivateReleaseSeriesError> {
    contract.verify_integrity()?;
    let input_file = PrivateFileIdentity {
        path: contract.input.path.clone(),
        bytes: contract.input.bytes,
        sha256: contract.input.sha256.clone(),
    };
    let current_executable = std::env::current_exe().map_err(|source| {
        error(format!(
            "resolve repository synthetic test executable: {source}"
        ))
    })?;
    if contract.purpose != "synthetic_mechanism"
        || contract.rom_class != ReleaseRomClass::Unclassified
        || contract.report_scenario != REPOSITORY_SYNTHETIC_RELEASE_SCENARIO
        || contract.guest_cycle != REPOSITORY_SYNTHETIC_RELEASE_CYCLE
        || contract.input.role != "synthetic_input"
        || contract.input.provenance != "repository_defined_synthetic"
        || !identity_matches_bytes(
            &contract.admission_manifest,
            REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES,
        )
        || !identity_matches_bytes(
            &contract.readiness_report,
            REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES,
        )
        || !identity_matches_bytes(&input_file, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES)
        || !contract.admitted_artifacts.is_empty()
        || contract.expected_execution_source != ExecutionDestinationSource::NoProgram
        || Path::new(&contract.child.executable.path) != current_executable
    {
        return Err(error(
            "synthetic runner authority is confined to fn64's exact repository-defined fixture, NoProgram source, and current test executable",
        ));
    }
    contract.verify_bound_files()?;
    Ok(VerifiedPrivateReleaseRunContract { contract })
}

pub fn run_private_release_series(
    verified_contract: &VerifiedPrivateReleaseRunContract,
    output_directory: impl AsRef<Path>,
) -> Result<PrivateReleaseSeriesReceipt, PrivateReleaseSeriesError> {
    let contract = verified_contract.contract();
    contract.verify_integrity()?;
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    let output_directory = output_directory.as_ref();
    validate_new_private_directory(output_directory)?;
    fs::create_dir(output_directory).map_err(|source| {
        error(format!(
            "create private release output directory {}: {source}",
            output_directory.display()
        ))
    })?;
    let staged_child = stage_child_executable(&contract.child.executable)?;
    let staged_microcode_pair = stage_microcode_pair(contract)?;

    let runner_executable = std::env::current_exe()
        .map_err(|source| error(format!("resolve runner executable: {source}")))?;
    let runner_executable_sha256 = sha256_file(&runner_executable, "runner executable")?.1;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce)
        .map_err(|source| error(format!("obtain OS-random private series nonce: {source}")))?;
    let nonce_hex = hex(&nonce);
    let child_executable_sha256 = contract.child.executable.sha256.clone();

    let mut evidence = Vec::with_capacity(PRIVATE_RELEASE_SERIES_COUNT);
    let mut runs = Vec::with_capacity(PRIVATE_RELEASE_SERIES_COUNT);
    for index in 0..PRIVATE_RELEASE_SERIES_COUNT {
        contract.verify_bound_files().map_err(|source| {
            error(format!(
                "private release preflight before child {} failed: {source}",
                index + 1
            ))
        })?;
        verify_release_program_build_receipt_binding(contract).map_err(|source| {
            error(format!(
                "private release program-build preflight before child {} failed: {source}",
                index + 1
            ))
        })?;
        if let Some(pair) = &staged_microcode_pair {
            pair.verify().map_err(|source| {
                error(format!(
                    "private release staged microcode preflight before child {} failed: {source}",
                    index + 1
                ))
            })?;
        }
        let staged_identity = PrivateFileIdentity {
            path: staged_child
                .0
                .to_str()
                .ok_or_else(|| error("staged child executable path is not UTF-8"))?
                .to_owned(),
            bytes: contract.child.executable.bytes,
            sha256: contract.child.executable.sha256.clone(),
        };
        verify_file_identity(&staged_identity, "staged child executable", false)?;
        let ordinal = u64::try_from(index + 1).expect("ten runs fit u64");
        let report_name = report_name(ordinal);
        let report_path = output_directory.join(&report_name);
        let journal_path = report_path.with_extension("unsupported.jsonl");
        let stdout_path = output_directory.join(format!("run-{ordinal:02}.stdout.log"));
        let stderr_path = output_directory.join(format!("run-{ordinal:02}.stderr.log"));
        for path in [&report_path, &journal_path, &stdout_path, &stderr_path] {
            if path.exists() || path.symlink_metadata().is_ok() {
                return Err(error(format!(
                    "private release child {ordinal} output {} already exists",
                    path.display()
                )));
            }
        }
        let run_event_sha256 = derive_run_event_sha256(
            &nonce,
            &contract.contract_sha256,
            &child_executable_sha256,
            ordinal,
            &report_name,
        )?;
        let stdout = create_new_file(&stdout_path, "child stdout log")?;
        let stderr = create_new_file(&stderr_path, "child stderr log")?;
        let mut command = Command::new(&staged_child.0);
        command
            .args(&contract.child.argv)
            .current_dir(&contract.child.working_directory)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        for entry in &contract.child.environment {
            command.env(&entry.name, &entry.value);
        }
        command
            .env("ROM", &contract.input.path)
            .env(RELEASE_ROM_CLASS_ENV, contract.rom_class.wire_name())
            .env(RELEASE_GATE_CYCLE_ENV, contract.guest_cycle.to_string())
            .env(RELEASE_REPORT_ENV, &report_path)
            .env(RELEASE_RUN_EVENT_SHA256_ENV, &run_event_sha256);
        if let Some(pair) = &staged_microcode_pair {
            command
                .env(RELEASE_MICROCODE_TEXT_PATH_ENV, &pair.text.0)
                .env(RELEASE_MICROCODE_DATA_PATH_ENV, &pair.data.0);
        }

        let status = command.status().map_err(|source| {
            error(format!(
                "spawn private release child {ordinal} directly from {}: {source}",
                staged_child.0.display()
            ))
        })?;
        if !status.success() {
            return Err(error(format!(
                "private release child {ordinal} exited unsuccessfully with {status}; outputs and journal were preserved in {}",
                output_directory.display()
            )));
        }

        let verified = read_and_verify_pair(
            contract,
            ordinal,
            &run_event_sha256,
            &report_path,
            &journal_path,
        )?;
        runs.push(PrivateReleaseSeriesRun {
            ordinal,
            run_event_sha256,
            report_file_sha256: verified.report_file_sha256,
            journal_file_sha256: verified.journal_file_sha256,
            report_sha256: verified.report.report_sha256.clone(),
            artifact_root_sha256: verified.report.digest.root_sha256.clone(),
        });
        evidence.push((verified.report, verified.journal));
    }
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    if let Some(pair) = &staged_microcode_pair {
        pair.verify()?;
    }
    let staged_identity = PrivateFileIdentity {
        path: staged_child
            .0
            .to_str()
            .ok_or_else(|| error("staged child executable path is not UTF-8"))?
            .to_owned(),
        bytes: contract.child.executable.bytes,
        sha256: contract.child.executable.sha256.clone(),
    };
    verify_file_identity(&staged_identity, "staged child executable", false)?;
    let verified = verify_release_evidence_series(&evidence, PRIVATE_RELEASE_SERIES_COUNT)
        .map_err(|source| error(format!("verify exact private release series: {source}")))?;
    if verified.count != PRIVATE_RELEASE_SERIES_COUNT
        || evidence.len() != PRIVATE_RELEASE_SERIES_COUNT
    {
        return Err(error(format!(
            "private release verifier returned {} reports; exactly {} are required",
            verified.count, PRIVATE_RELEASE_SERIES_COUNT
        )));
    }
    let mut receipt = PrivateReleaseSeriesReceipt {
        schema: PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA.to_owned(),
        contract_sha256: contract.contract_sha256.clone(),
        runner_executable_sha256,
        child_executable_sha256,
        series_nonce: nonce_hex,
        report_scenario: contract.report_scenario.clone(),
        input_sha256: contract.input.sha256.clone(),
        guest_cycle: contract.guest_cycle,
        expected_execution_source: contract.expected_execution_source.clone(),
        count: verified.count,
        semantic_report_sha256: verified.report_sha256,
        runs,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt.recompute_receipt_sha256()?;
    receipt.verify_integrity()?;
    write_receipt_new(&output_directory.join(RECEIPT_FILE), &receipt)?;
    Ok(receipt)
}

pub fn verify_private_release_series(
    verified_contract: &VerifiedPrivateReleaseRunContract,
    output_directory: impl AsRef<Path>,
    receipt: &PrivateReleaseSeriesReceipt,
) -> Result<VerifiedPrivateReleaseSeries, PrivateReleaseSeriesError> {
    let runner_executable = std::env::current_exe()
        .map_err(|source| error(format!("resolve verifier executable: {source}")))?;
    verify_private_release_series_with_runner(
        verified_contract,
        output_directory,
        receipt,
        runner_executable,
    )
}

/// Verify a retained series against the exact executable image that ran it.
///
/// This explicit runner path is required when verification occurs in a
/// different binary from `run-private-release-series`. The receipt binds only
/// the runner digest, so accepting no path here would turn that identity into
/// an unchecked caller assertion.
pub fn verify_private_release_series_with_runner(
    verified_contract: &VerifiedPrivateReleaseRunContract,
    output_directory: impl AsRef<Path>,
    receipt: &PrivateReleaseSeriesReceipt,
    runner_executable: impl AsRef<Path>,
) -> Result<VerifiedPrivateReleaseSeries, PrivateReleaseSeriesError> {
    let output_directory = output_directory.as_ref();
    let runner_executable = runner_executable.as_ref();
    verify_private_release_series_inner(
        verified_contract.contract(),
        output_directory,
        receipt,
        runner_executable,
    )?;
    Ok(VerifiedPrivateReleaseSeries {
        contract: verified_contract.contract().clone(),
        output_directory: output_directory.canonicalize().map_err(|source| {
            error(format!(
                "resolve private release output directory {}: {source}",
                output_directory.display()
            ))
        })?,
        receipt: receipt.clone(),
        runner_executable: runner_executable.canonicalize().map_err(|source| {
            error(format!(
                "resolve private release runner executable {}: {source}",
                runner_executable.display()
            ))
        })?,
    })
}

fn verify_private_release_series_inner(
    contract: &PrivateReleaseRunContract,
    output_directory: &Path,
    receipt: &PrivateReleaseSeriesReceipt,
    runner_executable: &Path,
) -> Result<(), PrivateReleaseSeriesError> {
    contract.verify_integrity()?;
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    receipt.verify_integrity()?;
    validate_private_existing_directory(output_directory, "private release output directory")?;
    let retained_receipt_path = output_directory.join(RECEIPT_FILE);
    validate_regular_no_symlink_file(&retained_receipt_path, "private release receipt")?;
    let retained_receipt: PrivateReleaseSeriesReceipt =
        serde_json::from_slice(&fs::read(&retained_receipt_path).map_err(|source| {
            error(format!(
                "read private release receipt {}: {source}",
                retained_receipt_path.display()
            ))
        })?)
        .map_err(|source| {
            error(format!(
                "parse private release receipt {}: {source}",
                retained_receipt_path.display()
            ))
        })?;
    if &retained_receipt != receipt {
        return Err(error(
            "supplied private release receipt differs from the exact receipt retained in its output directory",
        ));
    }
    if receipt.contract_sha256 != contract.contract_sha256
        || receipt.report_scenario != contract.report_scenario
        || receipt.input_sha256 != contract.input.sha256
        || receipt.guest_cycle != contract.guest_cycle
        || receipt.expected_execution_source != contract.expected_execution_source
        || receipt.child_executable_sha256 != contract.child.executable.sha256
    {
        return Err(error(
            "private release receipt does not match its exact run contract",
        ));
    }
    validate_regular_no_symlink_file(runner_executable, "private release runner executable")?;
    validate_native_executable(runner_executable)?;
    let runner_sha256 = sha256_file(runner_executable, "private release runner executable")?.1;
    if receipt.runner_executable_sha256 != runner_sha256 {
        return Err(error(format!(
            "private release receipt runner executable SHA {} differs from supplied runner {}",
            receipt.runner_executable_sha256, runner_sha256
        )));
    }
    let nonce = decode_hash(&receipt.series_nonce, "series_nonce")?;
    let mut evidence = Vec::with_capacity(PRIVATE_RELEASE_SERIES_COUNT);
    for run in &receipt.runs {
        let report_name = report_name(run.ordinal);
        let expected_event = derive_run_event_sha256(
            &nonce,
            &contract.contract_sha256,
            &contract.child.executable.sha256,
            run.ordinal,
            &report_name,
        )?;
        if run.run_event_sha256 != expected_event {
            return Err(error(format!(
                "receipt run {} event identity does not derive from its series nonce and contract",
                run.ordinal
            )));
        }
        let report_path = output_directory.join(&report_name);
        let journal_path = report_path.with_extension("unsupported.jsonl");
        let verified = read_and_verify_pair(
            contract,
            run.ordinal,
            &run.run_event_sha256,
            &report_path,
            &journal_path,
        )?;
        if verified.report_file_sha256 != run.report_file_sha256
            || verified.journal_file_sha256 != run.journal_file_sha256
            || verified.report.report_sha256 != run.report_sha256
            || verified.report.digest.root_sha256 != run.artifact_root_sha256
        {
            return Err(error(format!(
                "receipt run {} file or semantic identity differs from retained evidence",
                run.ordinal
            )));
        }
        evidence.push((verified.report, verified.journal));
    }
    let verified = verify_release_evidence_series(&evidence, PRIVATE_RELEASE_SERIES_COUNT)
        .map_err(|source| error(format!("reverify exact private release series: {source}")))?;
    if verified.count != PRIVATE_RELEASE_SERIES_COUNT
        || verified.report_sha256 != receipt.semantic_report_sha256
    {
        return Err(error(
            "retained private release evidence does not match its exact ten-run receipt",
        ));
    }
    contract.verify_bound_files()?;
    verify_release_program_build_receipt_binding(contract)?;
    Ok(())
}

struct VerifiedPair {
    report: ReleaseGateReport,
    journal: ParsedUnsupportedJournal,
    report_file_sha256: String,
    journal_file_sha256: String,
}

fn read_and_verify_pair(
    contract: &PrivateReleaseRunContract,
    ordinal: u64,
    expected_run_event_sha256: &str,
    report_path: &Path,
    journal_path: &Path,
) -> Result<VerifiedPair, PrivateReleaseSeriesError> {
    validate_regular_no_symlink_file(report_path, "release report")?;
    validate_regular_no_symlink_file(journal_path, "unsupported journal")?;
    let report_bytes = fs::read(report_path)
        .map_err(|source| error(format!("read report {}: {source}", report_path.display())))?;
    let journal_bytes = fs::read(journal_path)
        .map_err(|source| error(format!("read journal {}: {source}", journal_path.display())))?;
    let report_file_sha256 = sha256_hex(&report_bytes);
    let journal_file_sha256 = sha256_hex(&journal_bytes);
    let report: ReleaseGateReport = serde_json::from_slice(&report_bytes)
        .map_err(|source| error(format!("parse report {}: {source}", report_path.display())))?;
    if report.schema != RELEASE_REPORT_SCHEMA {
        return Err(error(format!(
            "private release child {ordinal} emitted report schema {:?}; exact {RELEASE_REPORT_SCHEMA} is required",
            report.schema
        )));
    }
    if report.scenario != contract.report_scenario
        || report.digest.guest_cycle != contract.guest_cycle
        || report.execution_destinations.source != contract.expected_execution_source
    {
        return Err(error(format!(
            "private release child {ordinal} report does not match contract scenario/cycle/execution source"
        )));
    }
    verify_report_rom_binding(contract, ordinal, &report)?;
    require_exact_artifacts(&report)?;
    let journal = parse_unsupported_journal(&journal_bytes).map_err(|source| {
        error(format!(
            "parse unsupported journal {}: {source}",
            journal_path.display()
        ))
    })?;
    verify_release_report_journal(&report, &journal).map_err(|source| {
        error(format!(
            "verify private release child {ordinal} report/journal: {source}"
        ))
    })?;
    verify_consumed_microcode_pair(contract, ordinal, &report)?;
    if journal.release_run_event_sha256() != Some(expected_run_event_sha256) {
        return Err(error(format!(
            "private release child {ordinal} journal does not bind its runner-derived event identity"
        )));
    }
    verify_release_evidence_series(&[(report.clone(), journal.clone())], 1).map_err(|source| {
        error(format!(
            "private release child {ordinal} omits live-minimum or zero-unsupported evidence: {source}"
        ))
    })?;
    Ok(VerifiedPair {
        report,
        journal,
        report_file_sha256,
        journal_file_sha256,
    })
}

fn verify_report_rom_binding(
    contract: &PrivateReleaseRunContract,
    ordinal: u64,
    report: &ReleaseGateReport,
) -> Result<(), PrivateReleaseSeriesError> {
    if report.input_sha256 != contract.input.sha256 {
        return Err(error(format!(
            "private release child {ordinal} report input SHA-256 does not match the contract ROM"
        )));
    }
    if contract.purpose == "synthetic_mechanism" {
        if report.rom.is_some() {
            return Err(error(format!(
                "private release child {ordinal} synthetic report fabricated ROM evidence"
            )));
        }
    } else {
        match &report.rom {
            Some(rom)
                if rom.class == contract.rom_class
                    && rom.byte_len == contract.input.bytes => {
                let input_bytes = fs::read(&contract.input.path).map_err(|source| {
                    error(format!(
                        "private release child {ordinal} reread admitted ROM for header verification: {source}"
                    ))
                })?;
                if u64::try_from(input_bytes.len()).ok() != Some(contract.input.bytes)
                    || sha256_hex(&input_bytes) != contract.input.sha256
                {
                    return Err(error(format!(
                        "private release child {ordinal} admitted ROM identity drifted before header verification"
                    )));
                }
                let configured_tv_type = match rom.configured_tv_type {
                    ReleaseTvStandard::Ntsc => fn64_runtime::TvType::Ntsc,
                    ReleaseTvStandard::Pal => fn64_runtime::TvType::Pal,
                    ReleaseTvStandard::Mpal => fn64_runtime::TvType::Mpal,
                };
                let expected = ReleaseRomEvidence::from_bytes(
                    &input_bytes,
                    contract.rom_class,
                    configured_tv_type,
                )
                .map_err(|source| {
                    error(format!(
                        "private release child {ordinal} admitted ROM header is invalid: {source}"
                    ))
                })?;
                if rom != &expected {
                    return Err(error(format!(
                        "private release child {ordinal} ROM evidence does not match the independently decoded admitted ROM"
                    )));
                }
            }
            Some(rom) => {
                return Err(error(format!(
                    "private release child {ordinal} ROM evidence class/length {:?}/{} does not match contract {}/{}",
                    rom.class,
                    rom.byte_len,
                    contract.rom_class.wire_name(),
                    contract.input.bytes
                )))
            }
            None => {
                return Err(error(format!(
                    "private release child {ordinal} omitted contract-bound ROM evidence"
                )))
            }
        }
    }
    Ok(())
}

fn verify_consumed_microcode_pair(
    contract: &PrivateReleaseRunContract,
    ordinal: u64,
    report: &ReleaseGateReport,
) -> Result<(), PrivateReleaseSeriesError> {
    if contract.purpose == "synthetic_mechanism" {
        return Ok(());
    }
    let text = required_artifact(contract, "microcode_text")?;
    let data = required_artifact(contract, "microcode_data")?;
    let matched = report.rsp_rdp.ordered.iter().any(|event| {
        matches!(
            &event.observation,
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                text_sha256,
                data_bytes,
                data_sha256,
                family: Some(_),
                ..
            } if text_sha256 == &text.sha256
                && u64::from(*data_bytes) == data.bytes
                && data_sha256 == &data.sha256
        )
    });
    if !matched {
        return Err(error(format!(
            "private release child {ordinal} has no single recognized microcode event binding the admitted text SHA-256 and exact data length/SHA-256"
        )));
    }
    Ok(())
}

fn require_exact_artifacts(report: &ReleaseGateReport) -> Result<(), PrivateReleaseSeriesError> {
    let observed: BTreeSet<_> = report
        .digest
        .artifacts
        .iter()
        .map(|artifact| artifact.kind)
        .collect();
    let expected = BTreeSet::from([
        ArtifactKind::Framebuffer,
        ArtifactKind::Audio,
        ArtifactKind::Memory,
        ArtifactKind::DeviceState,
        ArtifactKind::TimingTrace,
    ]);
    if report.digest.artifacts.len() != expected.len() || observed != expected {
        return Err(error(
            "private release report does not contain the exact five fixed-cycle artifacts",
        ));
    }
    let declared: BTreeSet<_> = report
        .closure
        .iter()
        .map(|path| path.name.as_str())
        .collect();
    let missing: Vec<_> = LIVE_MINIMUM_CLOSURE_PATHS
        .iter()
        .copied()
        .filter(|path| !declared.contains(path))
        .collect();
    if !missing.is_empty() {
        return Err(error(format!(
            "private release report omits live-minimum paths {missing:?}"
        )));
    }
    Ok(())
}

fn derive_run_event_sha256(
    nonce: &[u8; 32],
    contract_sha256: &str,
    executable_sha256: &str,
    ordinal: u64,
    report_name: &str,
) -> Result<String, PrivateReleaseSeriesError> {
    let mut wire = Vec::new();
    wire.extend_from_slice(RUN_EVENT_DOMAIN);
    wire.extend_from_slice(nonce);
    push_hash(&mut wire, contract_sha256, "contract_sha256")?;
    push_hash(&mut wire, executable_sha256, "child.executable.sha256")?;
    push_u64(&mut wire, ordinal);
    push_bytes(&mut wire, report_name.as_bytes());
    Ok(sha256_hex(&wire))
}

fn report_name(ordinal: u64) -> String {
    format!("report-{ordinal:02}.json")
}

fn validate_file_identity_shape(
    identity: &PrivateFileIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    if identity.bytes == 0 {
        return Err(error(format!("{field}.bytes must be positive")));
    }
    require_sha256(&identity.sha256, &format!("{field}.sha256"))?;
    validate_absolute_no_parent(Path::new(&identity.path), &format!("{field}.path"))
}

fn validate_artifact_identity_shape(
    identity: &PrivateArtifactIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    if !canonical_role(&identity.role) {
        return Err(error(format!(
            "{field}.role {:?} is not canonical",
            identity.role
        )));
    }
    if !canonical_role(&identity.provenance) {
        return Err(error(format!(
            "{field}.provenance {:?} is not canonical",
            identity.provenance
        )));
    }
    validate_file_identity_shape(
        &PrivateFileIdentity {
            path: identity.path.clone(),
            bytes: identity.bytes,
            sha256: identity.sha256.clone(),
        },
        field,
    )
}

fn validate_execution_source(
    source: &ExecutionDestinationSource,
) -> Result<(), PrivateReleaseSeriesError> {
    match source {
        ExecutionDestinationSource::NoProgram => Ok(()),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 }
        | ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            require_sha256(artifact_sha256, "expected_execution_source.artifact_sha256")
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            require_sha256(program_sha256, "expected_execution_source.program_sha256")?;
            require_sha256(
                dispatch_artifact_sha256,
                "expected_execution_source.dispatch_artifact_sha256",
            )
        }
    }
}

fn validate_environment_entry(
    entry: &PrivateEnvironmentEntry,
) -> Result<(), PrivateReleaseSeriesError> {
    let bytes = entry.name.as_bytes();
    if bytes.is_empty()
        || !(bytes[0] == b'_' || bytes[0].is_ascii_uppercase())
        || !bytes[1..]
            .iter()
            .all(|byte| *byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(error(format!(
            "child environment name {:?} must match [A-Z_][A-Z0-9_]*",
            entry.name
        )));
    }
    if entry.name.contains('=') || entry.name.contains('\0') || entry.value.contains('\0') {
        return Err(error(format!(
            "child environment entry {:?} contains a forbidden character",
            entry.name
        )));
    }
    if reserved_environment_name(&entry.name) {
        return Err(error(format!(
            "child environment entry {:?} is runner-owned and cannot be declared",
            entry.name
        )));
    }
    if dangerous_code_loading_environment_name(&entry.name) {
        return Err(error(format!(
            "child environment entry {:?} can alter loader or interpreter code selection and is forbidden",
            entry.name
        )));
    }
    Ok(())
}

fn reserved_environment_name(name: &str) -> bool {
    name == "ROM"
        || name.starts_with("FN64_RELEASE_")
        || name.starts_with("OOT_RELEASE_")
        || name.starts_with("FN64_PRIVATE_RUN_")
}

fn dangerous_code_loading_environment_name(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "PATHEXT"
            | "COMSPEC"
            | "BASH_ENV"
            | "ENV"
            | "SHELLOPTS"
            | "ZDOTDIR"
            | "GCONV_PATH"
            | "LOCPATH"
            | "NLSPATH"
            | "CLASSPATH"
            | "JAVA_TOOL_OPTIONS"
            | "JDK_JAVA_OPTIONS"
            | "_JAVA_OPTIONS"
            | "SSLKEYLOGFILE"
            | "NODE_OPTIONS"
            | "GBM_BACKEND"
            | "GALLIUM_DRIVER"
            | "EGL_PLATFORM"
    ) || [
        "LD_",
        "DYLD_",
        "PYTHON",
        "PERL",
        "RUBY",
        "NODE_",
        "LUA_",
        "TCL_",
        "DOTNET_",
        "MONO_",
        "POWERSHELL_",
        "GTK_",
        "QT_",
        "VK_",
        "LIBGL_",
        "MESA_",
        "__GL_",
        "D3D12SDK",
        "DXVK_",
        "VKD3D_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn validate_scenario(value: &str) -> Result<(), PrivateReleaseSeriesError> {
    let bytes = value.as_bytes();
    let canonical = (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        });
    if !canonical
        || (bytes.len() == 64
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)))
    {
        return Err(error(format!(
            "private release scenario {value:?} is not a canonical content-free label"
        )));
    }
    Ok(())
}

fn canonical_role(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn verify_private_file_identity(
    identity: &PrivateFileIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    verify_file_identity(identity, field, true)
}

fn verify_private_artifact_identity(
    identity: &PrivateArtifactIdentity,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    verify_file_identity(
        &PrivateFileIdentity {
            path: identity.path.clone(),
            bytes: identity.bytes,
            sha256: identity.sha256.clone(),
        },
        field,
        true,
    )
}

fn verify_file_identity(
    identity: &PrivateFileIdentity,
    field: &str,
    private: bool,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_file_identity_shape(identity, field)?;
    let path = Path::new(&identity.path);
    validate_regular_no_symlink_file(path, field)?;
    if private {
        require_outside_or_ignored(path, field)?;
    }
    let (bytes, sha256) = sha256_file(path, field)?;
    if bytes != identity.bytes || sha256 != identity.sha256 {
        return Err(error(format!(
            "{field} identity drift at {}: expected bytes={} sha256={}, observed bytes={bytes} sha256={sha256}",
            path.display(), identity.bytes, identity.sha256
        )));
    }
    Ok(())
}

fn sha256_file(path: &Path, field: &str) -> Result<(u64, String), PrivateReleaseSeriesError> {
    let mut file = File::open(path)
        .map_err(|source| error(format!("open {field} {}: {source}", path.display())))?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| error(format!("read {field} {}: {source}", path.display())))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read).expect("buffer read fits u64"))
            .ok_or_else(|| error(format!("{field} {} length overflow", path.display())))?;
        digest.update(&buffer[..read]);
    }
    Ok((total, hex(&digest.finalize())))
}

fn validate_absolute_no_parent(path: &Path, field: &str) -> Result<(), PrivateReleaseSeriesError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(error(format!(
            "{field} {} must be absolute and contain no '..' component",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_components(
    path: &Path,
    include_leaf: bool,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    let components: Vec<_> = path.components().collect();
    let limit = if include_leaf {
        components.len()
    } else {
        components.len().saturating_sub(1)
    };
    let mut current = PathBuf::new();
    for component in components.into_iter().take(limit) {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(error(format!(
                    "{field} has forbidden symlink component {}",
                    current.display()
                )))
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(error(format!(
                    "inspect {field} component {}: {source}",
                    current.display()
                )))
            }
        }
    }
    Ok(())
}

fn validate_regular_no_symlink_file(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_parent(path, field)?;
    reject_symlink_components(path, true, field)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {field} {}: {source}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(error(format!(
            "{field} {} is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn validate_native_executable(path: &Path) -> Result<(), PrivateReleaseSeriesError> {
    let mut file = File::open(path).map_err(|source| {
        error(format!(
            "open native executable candidate {}: {source}",
            path.display()
        ))
    })?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|source| {
        error(format!(
            "read native executable magic {}: {source}",
            path.display()
        ))
    })?;
    let elf = magic == *b"\x7fELF";
    let mach_o = matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xce]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca]
    );
    let portable_executable = if magic[..2] == *b"MZ" {
        file.seek(SeekFrom::Start(0x3c))
            .and_then(|_| {
                let mut offset = [0u8; 4];
                file.read_exact(&mut offset)?;
                file.seek(SeekFrom::Start(u64::from(u32::from_le_bytes(offset))))
            })
            .and_then(|_| {
                let mut signature = [0u8; 4];
                file.read_exact(&mut signature)?;
                Ok(signature == *b"PE\0\0")
            })
            .unwrap_or(false)
    } else {
        false
    };
    if !elf && !mach_o && !portable_executable {
        return Err(error(format!(
            "child executable {} is not a native ELF, Mach-O, or PE image; scripts and interpreter-mediated launchers are forbidden",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file
            .metadata()
            .map_err(|source| {
                error(format!(
                    "inspect native executable permissions {}: {source}",
                    path.display()
                ))
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(error(format!(
                "child executable {} has native image bytes but no Unix execute bit",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_absolute_no_symlink_directory(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_parent(path, field)?;
    reject_symlink_components(path, true, field)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| error(format!("inspect {field} {}: {source}", path.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(error(format!(
            "{field} {} is not a directory",
            path.display()
        )));
    }
    Ok(())
}

fn validate_private_existing_path(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_regular_no_symlink_file(path, field)?;
    require_outside_or_ignored(path, field)
}

fn validate_private_existing_directory(
    path: &Path,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_symlink_directory(path, field)?;
    require_outside_or_ignored(path, field)
}

fn validate_new_private_directory(path: &Path) -> Result<(), PrivateReleaseSeriesError> {
    validate_absolute_no_parent(path, "output directory")?;
    reject_symlink_components(path, false, "output directory")?;
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        return Err(error(format!(
            "private release output directory {} already exists",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| error("private release output directory has no parent"))?;
    validate_absolute_no_symlink_directory(parent, "output directory parent")?;
    require_outside_or_ignored(path, "output directory")
}

fn require_outside_or_ignored(path: &Path, field: &str) -> Result<(), PrivateReleaseSeriesError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|source| error(format!("resolve fn64 repository root: {source}")))?;
    let comparable = if path.exists() {
        path.canonicalize()
            .map_err(|source| error(format!("resolve {field} {}: {source}", path.display())))?
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| error(format!("{field} {} has no parent", path.display())))?
            .canonicalize()
            .map_err(|source| {
                error(format!(
                    "resolve {field} parent {}: {source}",
                    path.display()
                ))
            })?;
        parent.join(
            path.file_name()
                .ok_or_else(|| error(format!("{field} {} has no file name", path.display())))?,
        )
    };
    let Ok(relative) = comparable.strip_prefix(&root) else {
        return Ok(());
    };
    if Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative)
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Err(error(format!(
            "{field} {} is tracked by git; private evidence cannot enter the repository",
            path.display()
        )));
    }
    if !Command::new("git")
        .args(["check-ignore", "-q", "--no-index", "--"])
        .arg(relative)
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Err(error(format!(
            "{field} {} is inside the repository and not gitignored",
            path.display()
        )));
    }
    Ok(())
}

fn create_new_file(path: &Path, field: &str) -> Result<File, PrivateReleaseSeriesError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| error(format!("create {field} {}: {source}", path.display())))
}

fn write_receipt_new(
    path: &Path,
    receipt: &PrivateReleaseSeriesReceipt,
) -> Result<(), PrivateReleaseSeriesError> {
    let mut file = create_new_file(path, "private release receipt")?;
    serde_json::to_writer_pretty(&mut file, receipt)
        .map_err(|source| error(format!("serialize receipt {}: {source}", path.display())))?;
    file.write_all(b"\n")
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|source| error(format!("persist receipt {}: {source}", path.display())))?;
    drop(file);
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| error("private release receipt has no parent directory"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| {
                error(format!(
                    "persist private release receipt directory {}: {source}",
                    parent.display()
                ))
            })?;
    }
    Ok(())
}

fn encode_file_identity(
    wire: &mut Vec<u8>,
    value: &PrivateFileIdentity,
) -> Result<(), PrivateReleaseSeriesError> {
    push_bytes(wire, value.path.as_bytes());
    push_u64(wire, value.bytes);
    push_hash(wire, &value.sha256, "file identity sha256")
}

fn encode_artifact_identity(
    wire: &mut Vec<u8>,
    value: &PrivateArtifactIdentity,
) -> Result<(), PrivateReleaseSeriesError> {
    push_bytes(wire, value.role.as_bytes());
    push_bytes(wire, value.path.as_bytes());
    push_u64(wire, value.bytes);
    push_hash(wire, &value.sha256, "artifact identity sha256")?;
    push_bytes(wire, value.provenance.as_bytes());
    Ok(())
}

fn encode_execution_source(
    wire: &mut Vec<u8>,
    value: &ExecutionDestinationSource,
) -> Result<(), PrivateReleaseSeriesError> {
    match value {
        ExecutionDestinationSource::NoProgram => wire.push(0),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 } => {
            wire.push(1);
            push_hash(wire, artifact_sha256, "execution source artifact_sha256")?;
        }
        ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            wire.push(2);
            push_hash(wire, artifact_sha256, "execution source artifact_sha256")?;
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            wire.push(3);
            push_hash(wire, program_sha256, "execution source program_sha256")?;
            push_hash(
                wire,
                dispatch_artifact_sha256,
                "execution source dispatch_artifact_sha256",
            )?;
        }
    }
    Ok(())
}

fn push_u64(wire: &mut Vec<u8>, value: u64) {
    wire.extend_from_slice(&value.to_be_bytes());
}

fn push_bytes(wire: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(
        wire,
        u64::try_from(bytes.len()).expect("host byte slice length fits canonical u64"),
    );
    wire.extend_from_slice(bytes);
}

fn push_hash(
    wire: &mut Vec<u8>,
    value: &str,
    field: &str,
) -> Result<(), PrivateReleaseSeriesError> {
    wire.extend_from_slice(&decode_hash(value, field)?);
    Ok(())
}

fn require_sha256(value: &str, field: &str) -> Result<(), PrivateReleaseSeriesError> {
    decode_hash(value, field).map(|_| ())
}

fn decode_hash(value: &str, field: &str) -> Result<[u8; 32], PrivateReleaseSeriesError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(format!("{field} is not a lowercase SHA-256")));
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16).expect("validated hex") as u8;
        let low = (pair[1] as char).to_digit(16).expect("validated hex") as u8;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn error(message: impl Into<String>) -> PrivateReleaseSeriesError {
    PrivateReleaseSeriesError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClosurePath, ClosurePathStatus, FixedCycleDigestGate, ReleaseObservationGeometry};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    const FIXTURE_ENV: &str = "FN64_TEST_RELEASE_CHILD";
    const TEMPLATE_ENV: &str = "FN64_TEST_RELEASE_TEMPLATE";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let base = if Path::new("/private/tmp").is_dir() {
                PathBuf::from("/private/tmp")
            } else {
                std::env::temp_dir()
            };
            let path = base.join(format!(
                "fn64-private-release-series-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file_identity(path: &Path) -> PrivateFileIdentity {
        let (bytes, sha256) = sha256_file(path, "test file").unwrap();
        PrivateFileIdentity {
            path: path.to_str().unwrap().to_owned(),
            bytes,
            sha256,
        }
    }

    fn artifact_identity(path: &Path, role: &str) -> PrivateArtifactIdentity {
        let file = file_identity(path);
        PrivateArtifactIdentity {
            role: role.to_owned(),
            path: file.path,
            bytes: file.bytes,
            sha256: file.sha256,
            provenance: "repository_defined_synthetic".to_owned(),
        }
    }

    fn fixture_report(input: &[u8], source: ExecutionDestinationSource) -> ReleaseGateReport {
        let cycle = REPOSITORY_SYNTHETIC_RELEASE_CYCLE;
        let mut digest = FixedCycleDigestGate::new(cycle);
        digest
            .capture(cycle, ArtifactKind::Framebuffer, &[0, 1])
            .unwrap();
        for kind in [
            ArtifactKind::Audio,
            ArtifactKind::DeviceState,
            ArtifactKind::TimingTrace,
        ] {
            digest.capture(cycle, kind, &[kind as u8]).unwrap();
        }
        digest
            .capture(
                cycle,
                ArtifactKind::Memory,
                &vec![0; crate::DEFAULT_RDRAM_SIZE],
            )
            .unwrap();
        let closure = LIVE_MINIMUM_CLOSURE_PATHS
            .iter()
            .map(|name| ClosurePath {
                name: (*name).to_owned(),
                observations: 1,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            })
            .collect::<Vec<_>>();
        let base = ReleaseGateReport::new(
            REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
            input,
            digest.finish().unwrap(),
            ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            closure.clone(),
        )
        .unwrap();
        let destinations =
            crate::ExecutionDestinationEvidence::from_ordered(source, Vec::new()).unwrap();
        ReleaseGateReport::new_with_test_environment_and_destinations(
            REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
            input,
            base.digest,
            base.observations,
            base.environment,
            destinations,
            closure,
        )
        .unwrap()
    }

    fn fixture_contract(directory: &Path) -> (PrivateReleaseRunContract, PathBuf) {
        fs::create_dir_all(directory).unwrap();
        let manifest = directory.join("manifest.json");
        let readiness = directory.join("readiness.json");
        let input = directory.join("synthetic-input.bin");
        fs::write(&manifest, REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES).unwrap();
        fs::write(&readiness, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES).unwrap();
        fs::write(&input, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES).unwrap();
        let source = ExecutionDestinationSource::NoProgram;
        let report = fixture_report(&fs::read(&input).unwrap(), source.clone());
        let template = directory.join("report-template.json");
        report.write_json(&template).unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut contract = PrivateReleaseRunContract {
            schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
            admission_manifest: file_identity(&manifest),
            readiness_report: file_identity(&readiness),
            program_build_receipt: None,
            purpose: "synthetic_mechanism".to_owned(),
            rom_class: ReleaseRomClass::Unclassified,
            report_scenario: REPOSITORY_SYNTHETIC_RELEASE_SCENARIO.to_owned(),
            guest_cycle: REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
            repeat_count: PRIVATE_RELEASE_SERIES_COUNT,
            input: artifact_identity(&input, "synthetic_input"),
            admitted_artifacts: Vec::new(),
            expected_execution_source: source,
            child: PrivateChildCommand {
                executable: file_identity(&executable),
                working_directory: directory.to_str().unwrap().to_owned(),
                argv: vec![
                    "--exact".to_owned(),
                    "private_release_series::tests::fresh_child_fixture".to_owned(),
                    "--nocapture".to_owned(),
                ],
                environment: vec![
                    PrivateEnvironmentEntry {
                        name: FIXTURE_ENV.to_owned(),
                        value: "1".to_owned(),
                    },
                    PrivateEnvironmentEntry {
                        name: TEMPLATE_ENV.to_owned(),
                        value: template.to_str().unwrap().to_owned(),
                    },
                ],
            },
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = contract.recompute_contract_sha256().unwrap();
        (contract, template)
    }

    #[test]
    fn fresh_child_fixture() {
        if std::env::var_os(FIXTURE_ENV).is_none() {
            return;
        }
        let template = PathBuf::from(std::env::var(TEMPLATE_ENV).unwrap());
        let report_path = PathBuf::from(std::env::var(RELEASE_REPORT_ENV).unwrap());
        let cycle = std::env::var(RELEASE_GATE_CYCLE_ENV).unwrap();
        let event = std::env::var(RELEASE_RUN_EVENT_SHA256_ENV).unwrap();
        let report: ReleaseGateReport =
            serde_json::from_slice(&fs::read(&template).unwrap()).unwrap();
        fs::copy(template, &report_path).unwrap();
        let journal = report_path.with_extension("unsupported.jsonl");
        fs::write(
            journal,
            format!(
                "fn64.unsupported-journal.v3\tarmed\t{event}\nfn64.unsupported-journal.v3\tcomplete\t{cycle}\t{}\t{event}\n",
                report.report_sha256
            ),
        )
        .unwrap();
    }

    #[test]
    fn launches_and_reverifies_ten_fresh_child_processes() {
        let directory = TestDirectory::new();
        let (contract, non_runner) = fixture_contract(&directory.0);
        let contract = verify_repository_synthetic_private_release_run_contract(contract).unwrap();
        let output = directory.0.join("series");
        let receipt = run_private_release_series(&contract, &output).unwrap();
        assert_eq!(receipt.count, PRIVATE_RELEASE_SERIES_COUNT);
        assert_eq!(receipt.runs.len(), PRIVATE_RELEASE_SERIES_COUNT);
        assert_eq!(
            receipt
                .runs
                .iter()
                .map(|run| &run.run_event_sha256)
                .collect::<BTreeSet<_>>()
                .len(),
            PRIVATE_RELEASE_SERIES_COUNT
        );
        let verified_series = verify_private_release_series(&contract, &output, &receipt).unwrap();
        verified_series.revalidate_for_release_matrix().unwrap();
        let retained: PrivateReleaseSeriesReceipt =
            serde_json::from_slice(&fs::read(output.join(RECEIPT_FILE)).unwrap()).unwrap();
        assert_eq!(retained, receipt);

        let mut substituted_receipt = receipt.clone();
        substituted_receipt.runner_executable_sha256 = "aa".repeat(32);
        substituted_receipt.receipt_sha256 =
            substituted_receipt.recompute_receipt_sha256().unwrap();
        assert!(verify_private_release_series_with_runner(
            &contract,
            &output,
            &substituted_receipt,
            std::env::current_exe().unwrap(),
        )
        .unwrap_err()
        .to_string()
        .contains("differs from the exact receipt retained"));
        assert!(verify_private_release_series_with_runner(
            &contract,
            &output,
            &receipt,
            &non_runner,
        )
        .unwrap_err()
        .to_string()
        .contains("not a native"));

        fs::write(output.join(report_name(1)), b"retained report drift").unwrap();
        assert!(verified_series
            .revalidate_for_release_matrix()
            .unwrap_err()
            .to_string()
            .contains("parse report"));
    }

    #[test]
    fn admission_policy_receives_the_exact_bytes_rust_will_parse() {
        let directory = TestDirectory::new();
        let source = directory.0.join("contract.json");
        fs::write(&source, b"source pathname may change after capture").unwrap();
        let captured = b"exact captured contract bytes";
        let staged_path;
        {
            let staged = stage_contract_for_admission(&source, captured).unwrap();
            staged_path = staged.0.clone();
            assert_ne!(staged_path, source);
            assert_eq!(fs::read(&staged_path).unwrap(), captured);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    fs::metadata(&staged_path).unwrap().permissions().mode() & 0o777,
                    0o400
                );
            }
        }
        assert!(!staged_path.exists());
    }

    #[test]
    fn child_stage_is_an_exact_independent_inode() {
        let directory = TestDirectory::new();
        let source = directory.0.join("synthetic-native-image");
        fs::write(&source, b"\x7fELFfixed repository bytes").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&source).unwrap().permissions();
            permissions.set_mode(permissions.mode() | 0o700);
            fs::set_permissions(&source, permissions).unwrap();
        }
        let identity = file_identity(&source);
        let staged = stage_child_executable(&identity).unwrap();
        let staged_path = staged.0.clone();
        assert_eq!(fs::read(&staged_path).unwrap(), fs::read(&source).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&staged_path).unwrap().permissions().mode() & 0o777,
                0o500
            );
        }

        fs::write(&source, b"\x7fELFmutated source bytes").unwrap();
        let staged_identity = PrivateFileIdentity {
            path: staged_path.to_str().unwrap().to_owned(),
            bytes: identity.bytes,
            sha256: identity.sha256,
        };
        verify_file_identity(&staged_identity, "test staged child", false).unwrap();
        drop(staged);
        assert!(!staged_path.exists());
    }

    #[test]
    fn microcode_pair_stage_is_exact_independent_and_revalidated() {
        let directory = TestDirectory::new();
        let text_path = directory.0.join("microcode-text.bin");
        let data_path = directory.0.join("microcode-data.bin");
        fs::write(&text_path, vec![0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]).unwrap();
        fs::write(&data_path, b"exact task data").unwrap();

        let (mut contract, _) = fixture_contract(&directory.0.join("contract"));
        contract.purpose = "full_rom".to_owned();
        contract.admitted_artifacts = vec![
            artifact_identity(&data_path, "microcode_data"),
            artifact_identity(&text_path, "microcode_text"),
        ];
        let pair = stage_microcode_pair(&contract).unwrap().unwrap();
        let staged_text_path = pair.text.0.clone();
        let staged_data_path = pair.data.0.clone();
        assert_ne!(staged_text_path, text_path);
        assert_ne!(staged_data_path, data_path);
        assert_eq!(
            fs::read(&staged_text_path).unwrap(),
            vec![0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]
        );
        assert_eq!(fs::read(&staged_data_path).unwrap(), b"exact task data");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&staged_text_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
            assert_eq!(
                fs::metadata(&staged_data_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
        }

        fs::write(&text_path, vec![0xa5; fn64_runtime::RSP_MEMORY_BANK_SIZE]).unwrap();
        fs::write(&data_path, b"mutated source").unwrap();
        pair.verify().unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&staged_data_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut permissions = fs::metadata(&staged_data_path).unwrap().permissions();
            permissions.set_readonly(false);
            fs::set_permissions(&staged_data_path, permissions).unwrap();
        }
        fs::write(&staged_data_path, b"tampered staged data").unwrap();
        assert!(pair
            .verify()
            .unwrap_err()
            .to_string()
            .contains("identity drift"));
        drop(pair);
        assert!(!staged_text_path.exists());
        assert!(!staged_data_path.exists());
    }

    #[test]
    fn contract_rejects_shrunk_no_program_and_reserved_environment() {
        let directory = TestDirectory::new();
        let (contract, _) = fixture_contract(&directory.0);

        let mut shrunk = contract.clone();
        shrunk.repeat_count = 9;
        shrunk.contract_sha256 = shrunk.recompute_contract_sha256().unwrap();
        assert!(shrunk
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("exactly 10"));

        let mut no_program = contract.clone();
        no_program.purpose = "full_rom".to_owned();
        no_program.input.role = "rom".to_owned();
        no_program.expected_execution_source = ExecutionDestinationSource::NoProgram;
        no_program.contract_sha256 = no_program.recompute_contract_sha256().unwrap();
        assert!(no_program
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("authoritative executable"));

        for name in [
            "FN64_PRIVATE_RUN_ID",
            RELEASE_MICROCODE_TEXT_PATH_ENV,
            RELEASE_MICROCODE_DATA_PATH_ENV,
            RELEASE_ROM_CLASS_ENV,
        ] {
            let mut reserved = contract.clone();
            reserved.child.environment.push(PrivateEnvironmentEntry {
                name: name.to_owned(),
                value: "forged".to_owned(),
            });
            reserved
                .child
                .environment
                .sort_by(|left, right| left.name.cmp(&right.name));
            reserved.contract_sha256 = reserved.recompute_contract_sha256().unwrap();
            assert!(reserved
                .verify_integrity()
                .unwrap_err()
                .to_string()
                .contains("runner-owned"));
        }
    }

    #[test]
    fn repository_synthetic_authority_and_code_loading_fail_closed() {
        let directory = TestDirectory::new();
        let (contract, _) = fixture_contract(&directory.0);

        let mut relabelled = contract.clone();
        relabelled.rom_class = ReleaseRomClass::RetailCartridge;
        relabelled.contract_sha256 = relabelled.recompute_contract_sha256().unwrap();
        assert!(relabelled
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("must be unclassified"));

        let mut production = contract.clone();
        production.purpose = "full_rom".to_owned();
        production.input.role = "rom".to_owned();
        production.contract_sha256 = production.recompute_contract_sha256().unwrap();
        assert!(verify_release_program_build_receipt_binding(&production)
            .unwrap_err()
            .to_string()
            .contains(RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA));
        assert!(verify_repository_synthetic_private_release_run_contract(production).is_err());

        let (mut relabelled, _) = fixture_contract(&directory.0.join("relabelled"));
        let input_path = PathBuf::from(&relabelled.input.path);
        fs::write(&input_path, b"caller-labelled non-fixture bytes").unwrap();
        let input = artifact_identity(&input_path, "synthetic_input");
        relabelled.input = input;
        relabelled.contract_sha256 = relabelled.recompute_contract_sha256().unwrap();
        assert!(
            verify_repository_synthetic_private_release_run_contract(relabelled)
                .unwrap_err()
                .to_string()
                .contains("exact repository-defined fixture")
        );

        for name in [
            "PATH",
            "BASH_ENV",
            "SHELLOPTS",
            "GCONV_PATH",
            "SSLKEYLOGFILE",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "PYTHONPATH",
            "PERL5LIB",
            "RUBYOPT",
            "NODE_OPTIONS",
            "LUA_PATH",
            "DOTNET_STARTUP_HOOKS",
            "GTK_PATH",
            "QT_PLUGIN_PATH",
            "VK_LAYER_PATH",
            "LIBGL_DRIVERS_PATH",
            "GBM_BACKEND",
            "GALLIUM_DRIVER",
            "EGL_PLATFORM",
            "D3D12SDK_PATH",
            "DXVK_CONFIG_FILE",
            "VKD3D_CONFIG",
        ] {
            let entry = PrivateEnvironmentEntry {
                name: name.to_owned(),
                value: "injected".to_owned(),
            };
            assert!(validate_environment_entry(&entry)
                .unwrap_err()
                .to_string()
                .contains("loader or interpreter"));
        }

        let script = directory.0.join("child-script");
        fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
        assert!(validate_native_executable(&script)
            .unwrap_err()
            .to_string()
            .contains("not a native"));
    }

    #[test]
    fn repository_admission_script_matches_compiled_runner_bytes() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/private_input_admission.py");
        assert!(
            fs::read(path).unwrap() == PRIVATE_INPUT_ADMISSION_SCRIPT,
            "runtime admission script differs from runner-embedded bytes"
        );

        let directory = TestDirectory::new();
        let invalid = directory.0.join("invalid-contract.json");
        fs::write(&invalid, b"{}\n").unwrap();
        assert!(load_private_release_run_contract(&invalid)
            .unwrap_err()
            .to_string()
            .contains("repository private input admission verifier rejected"));
    }

    #[cfg(unix)]
    #[test]
    fn python_emitted_production_contract_authorizes_the_same_build_receipt_in_rust() {
        let directory = TestDirectory::new();
        let manifest_path = directory.0.join("manifest.json");
        let readiness_path = directory.0.join("readiness.json");
        let contract_path = directory.0.join("contract.json");
        let text = directory.0.join("microcode-text.bin");
        let data = directory.0.join("microcode-data.bin");
        let rom = directory.0.join("synthetic-rom.bin");
        let recompiled = directory.0.join("synthetic-recompiled.bin");
        let program_receipt_path = directory.0.join("program-build-receipt.json");
        fs::write(&text, vec![0x5a; 4096]).unwrap();
        fs::write(&data, vec![0xa5; 257]).unwrap();
        fs::write(&rom, b"repository non-game synthetic ROM stand-in").unwrap();
        fs::write(
            &recompiled,
            b"repository non-game synthetic recompiled stand-in",
        )
        .unwrap();
        let executable = Path::new("/usr/bin/true").canonicalize().unwrap();
        let recompiled_identity = file_identity(&recompiled);
        let executable_identity = file_identity(&executable);
        let expected_execution_source = ExecutionDestinationSource::TypedBlockProgram {
            program_sha256: "11".repeat(32),
            dispatch_artifact_sha256: recompiled_identity.sha256.clone(),
        };
        let mut program_receipt = ReleaseProgramBuildReceipt {
            schema: RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
            child_executable: ReleaseProgramFileIdentity {
                path: executable_identity.path.clone(),
                bytes: executable_identity.bytes,
                sha256: executable_identity.sha256.clone(),
            },
            lane: ReleaseProgramBuildLane::TypedBlock {
                pack: ReleaseProgramFileIdentity {
                    path: recompiled_identity.path.clone(),
                    bytes: recompiled_identity.bytes,
                    sha256: recompiled_identity.sha256.clone(),
                },
                expected_program_sha256: "11".repeat(32),
            },
            expected_execution_source: expected_execution_source.clone(),
            receipt_sha256: String::new(),
        };
        program_receipt.receipt_sha256 = program_receipt.recompute_receipt_sha256().unwrap();
        fs::write(
            &program_receipt_path,
            serde_json::to_vec_pretty(&program_receipt).unwrap(),
        )
        .unwrap();

        let descriptor = |path: &Path, provenance: &str| {
            let identity = file_identity(path);
            serde_json::json!({
                "path": identity.path,
                "length": identity.bytes,
                "sha256": identity.sha256,
                "provenance": provenance,
                "git_identity": "excluded",
            })
        };
        let program_receipt_identity = file_identity(&program_receipt_path);
        let manifest = serde_json::json!({
            "schema": "fn64.private-input-admission.v7",
            "purpose": "full_rom",
            "intent": {
                "wire_family": "full_rom_mixed",
                "report_scenario": "synthetic-python-rust-policy-parity",
                "recognition": "runtime_must_confirm_backend_known_pair",
                "extended_gbi_cases": [],
                "characterization_suite": null,
                "program_evidence_lane": "typed_block_program",
                "rom_class": "retail_cartridge",
            },
            "release_matrix": {
                "platform": "macos_arm64",
                "controllers": ["standard_controller"],
                "save": "no_cartridge_save",
                "renderers": ["reference_lle_accuracy"],
                "repeat_bar": 10,
            },
            "artifacts": {
                "microcode_text": descriptor(&text, "user_owned_rom_derived"),
                "microcode_data": descriptor(&data, "user_owned_rom_derived"),
                "microcode_text_raw_window": null,
                "microcode_data_raw_window": null,
                "rom": descriptor(&rom, "user_owned_retail_cartridge_dump"),
                "recompiled": descriptor(&recompiled, "user_generated_from_owned_rom"),
            },
            "runner": {
                "executable": {
                    "path": executable_identity.path,
                    "length": executable_identity.bytes,
                    "sha256": executable_identity.sha256,
                    "git_identity": "excluded",
                },
                "working_directory": directory.0.to_str().unwrap(),
                "argv": ["--exact", "unused-private-release-child"],
                "env": {"FN64_SYNTHETIC_FIXED": "1"},
                "release_gate_cycle": REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
                "execution_source": {
                    "kind": "typed_block_program",
                    "program_sha256": "11".repeat(32),
                    "dispatch_artifact_sha256": recompiled_identity.sha256,
                },
                "program_build_receipt": {
                    "path": program_receipt_identity.path,
                    "length": program_receipt_identity.bytes,
                    "sha256": program_receipt_identity.sha256,
                    "git_identity": "excluded",
                },
            },
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/private_input_admission.py");
        let status = Command::new(SYSTEM_PYTHON3)
            .arg(&script)
            .arg("--manifest")
            .arg(&manifest_path)
            .arg("--report")
            .arg(&readiness_path)
            .arg("--emit-private-run-contract")
            .arg(&contract_path)
            .status()
            .unwrap();
        assert!(status.success());

        load_private_release_run_contract(&contract_path).unwrap();
    }

    #[test]
    fn production_report_requires_one_recognized_event_with_the_exact_admitted_pair() {
        let directory = TestDirectory::new();
        let (mut contract, _) = fixture_contract(&directory.0);
        contract.purpose = "full_rom".to_owned();
        contract.input.role = "rom".to_owned();
        contract.admitted_artifacts = vec![
            PrivateArtifactIdentity {
                role: "microcode_data".to_owned(),
                path: "/private/ucode.data".to_owned(),
                bytes: 257,
                sha256: "22".repeat(32),
                provenance: "user_owned_rom_derived".to_owned(),
            },
            PrivateArtifactIdentity {
                role: "microcode_text".to_owned(),
                path: "/private/ucode.text".to_owned(),
                bytes: fn64_runtime::RSP_MEMORY_BANK_SIZE as u64,
                sha256: "11".repeat(32),
                provenance: "user_owned_rom_derived".to_owned(),
            },
            PrivateArtifactIdentity {
                role: "recompiled".to_owned(),
                path: "/private/program.pack".to_owned(),
                bytes: 1,
                sha256: "33".repeat(32),
                provenance: "user_generated_from_owned_rom".to_owned(),
            },
        ];
        let mut report = fixture_report(b"ignored", ExecutionDestinationSource::NoProgram);
        report.rsp_rdp =
            crate::RspRdpEvidence::from_ordered(vec![crate::RspRdpObservationEventEvidence {
                guest_cycle: 42,
                observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                    task_address: 0x1000,
                    imem_generation: 1,
                    text_sha256: "11".repeat(32),
                    data_address: 0x2000,
                    data_bytes: 257,
                    data_sha256: "22".repeat(32),
                    family: Some(crate::ReleaseMicrocodeFamily::F3dzex2),
                },
            }])
            .unwrap();
        verify_consumed_microcode_pair(&contract, 1, &report).unwrap();

        let matching = report.rsp_rdp.ordered[0].clone();
        let mut split = report.clone();
        let mut text_only = matching.clone();
        let RspRdpObservationKindEvidence::MicrocodeRecognition { data_sha256, .. } =
            &mut text_only.observation
        else {
            unreachable!()
        };
        *data_sha256 = "44".repeat(32);
        let mut data_only = matching;
        let RspRdpObservationKindEvidence::MicrocodeRecognition { text_sha256, .. } =
            &mut data_only.observation
        else {
            unreachable!()
        };
        *text_sha256 = "55".repeat(32);
        split.rsp_rdp = crate::RspRdpEvidence::from_ordered(vec![text_only, data_only]).unwrap();
        assert!(verify_consumed_microcode_pair(&contract, 2, &split)
            .unwrap_err()
            .to_string()
            .contains("no single recognized microcode event"));

        let RspRdpObservationKindEvidence::MicrocodeRecognition { family, .. } =
            &mut report.rsp_rdp.ordered[0].observation
        else {
            unreachable!()
        };
        *family = None;
        assert!(verify_consumed_microcode_pair(&contract, 3, &report).is_err());
    }

    #[test]
    fn production_report_rom_binding_rejects_class_length_and_input_relabels() {
        let directory = TestDirectory::new();
        let (mut contract, _) = fixture_contract(&directory.0);
        let mut rom_bytes = vec![0u8; 0x40];
        rom_bytes[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        rom_bytes[0x3e] = b'E';
        contract.purpose = "full_rom".to_owned();
        contract.rom_class = ReleaseRomClass::RetailCartridge;
        contract.input.role = "rom".to_owned();
        contract.input.bytes = rom_bytes.len() as u64;
        contract.input.sha256 = sha256_hex(&rom_bytes);
        contract.input.provenance = "user_owned_retail_cartridge_dump".to_owned();
        fs::write(&contract.input.path, &rom_bytes).unwrap();

        let mut report = fixture_report(&rom_bytes, ExecutionDestinationSource::NoProgram);
        report.rom = Some(
            crate::ReleaseRomEvidence::from_bytes(
                &rom_bytes,
                ReleaseRomClass::RetailCartridge,
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap(),
        );
        verify_report_rom_binding(&contract, 1, &report).unwrap();

        let mut relabelled = report.clone();
        relabelled.rom.as_mut().unwrap().class = ReleaseRomClass::PublicHomebrew;
        assert!(verify_report_rom_binding(&contract, 2, &relabelled)
            .unwrap_err()
            .to_string()
            .contains("class/length"));

        let mut resized = report.clone();
        resized.rom.as_mut().unwrap().byte_len += 4;
        assert!(verify_report_rom_binding(&contract, 3, &resized).is_err());

        let mut forged_header_identity = report.clone();
        forged_header_identity
            .rom
            .as_mut()
            .unwrap()
            .canonical_sha256 = "11".repeat(32);
        assert!(
            verify_report_rom_binding(&contract, 4, &forged_header_identity)
                .unwrap_err()
                .to_string()
                .contains("independently decoded")
        );

        let mut other_input = report;
        other_input.input_sha256 = "00".repeat(32);
        assert!(verify_report_rom_binding(&contract, 5, &other_input)
            .unwrap_err()
            .to_string()
            .contains("input SHA-256"));
    }

    #[test]
    fn canonical_wires_bind_order_context_and_receipt_tamper() {
        let directory = TestDirectory::new();
        let (contract, _) = fixture_contract(&directory.0);
        let baseline = contract.recompute_contract_sha256().unwrap();
        let mut changed = contract.clone();
        changed.child.argv.push("changed".to_owned());
        assert_ne!(changed.recompute_contract_sha256().unwrap(), baseline);
        let mut relabelled = contract.clone();
        relabelled.rom_class = ReleaseRomClass::RetailCartridge;
        assert_ne!(relabelled.recompute_contract_sha256().unwrap(), baseline);

        let nonce = [0x5a; 32];
        let first = derive_run_event_sha256(
            &nonce,
            &contract.contract_sha256,
            &contract.child.executable.sha256,
            1,
            "report-01.json",
        )
        .unwrap();
        let second = derive_run_event_sha256(
            &nonce,
            &contract.contract_sha256,
            &contract.child.executable.sha256,
            2,
            "report-02.json",
        )
        .unwrap();
        assert_ne!(first, second);

        let runs = (1..=PRIVATE_RELEASE_SERIES_COUNT)
            .map(|ordinal| PrivateReleaseSeriesRun {
                ordinal: ordinal as u64,
                run_event_sha256: format!("{ordinal:064x}"),
                report_file_sha256: "22".repeat(32),
                journal_file_sha256: "33".repeat(32),
                report_sha256: "44".repeat(32),
                artifact_root_sha256: "55".repeat(32),
            })
            .collect();
        let mut receipt = PrivateReleaseSeriesReceipt {
            schema: PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA.to_owned(),
            contract_sha256: contract.contract_sha256,
            runner_executable_sha256: "66".repeat(32),
            child_executable_sha256: contract.child.executable.sha256,
            series_nonce: "77".repeat(32),
            report_scenario: contract.report_scenario,
            input_sha256: contract.input.sha256,
            guest_cycle: contract.guest_cycle,
            expected_execution_source: contract.expected_execution_source,
            count: PRIVATE_RELEASE_SERIES_COUNT,
            semantic_report_sha256: "44".repeat(32),
            runs,
            receipt_sha256: String::new(),
        };
        receipt.receipt_sha256 = receipt.recompute_receipt_sha256().unwrap();
        receipt.verify_integrity().unwrap();
        receipt.runs.swap(0, 1);
        assert!(receipt.verify_integrity().is_err());
    }

    #[test]
    fn contract_wire_matches_cross_language_golden() {
        let contract = PrivateReleaseRunContract {
            schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
            admission_manifest: PrivateFileIdentity {
                path: "/private/manifest.json".to_owned(),
                bytes: 123,
                sha256: "00".repeat(32),
            },
            readiness_report: PrivateFileIdentity {
                path: "/private/readiness.json".to_owned(),
                bytes: 456,
                sha256: "11".repeat(32),
            },
            program_build_receipt: Some(PrivateFileIdentity {
                path: "/private/program-build-receipt.json".to_owned(),
                bytes: 654,
                sha256: "12".repeat(32),
            }),
            purpose: "full_rom".to_owned(),
            rom_class: ReleaseRomClass::RetailCartridge,
            report_scenario: "canonical-wire-fixture".to_owned(),
            guest_cycle: 42,
            repeat_count: 10,
            input: PrivateArtifactIdentity {
                role: "rom".to_owned(),
                path: "/private/game.z64".to_owned(),
                bytes: 67_108_864,
                sha256: "22".repeat(32),
                provenance: "user_owned_retail_cartridge_dump".to_owned(),
            },
            admitted_artifacts: vec![
                PrivateArtifactIdentity {
                    role: "microcode_data".to_owned(),
                    path: "/private/ucode.data".to_owned(),
                    bytes: 128,
                    sha256: "33".repeat(32),
                    provenance: "user_owned_rom_derived".to_owned(),
                },
                PrivateArtifactIdentity {
                    role: "microcode_text".to_owned(),
                    path: "/private/ucode.text".to_owned(),
                    bytes: fn64_runtime::RSP_MEMORY_BANK_SIZE as u64,
                    sha256: "34".repeat(32),
                    provenance: "user_owned_rom_derived".to_owned(),
                },
                PrivateArtifactIdentity {
                    role: "recompiled".to_owned(),
                    path: "/private/game.a".to_owned(),
                    bytes: 789,
                    sha256: "44".repeat(32),
                    provenance: "user_generated_from_owned_rom".to_owned(),
                },
            ],
            expected_execution_source: ExecutionDestinationSource::TypedBlockProgram {
                program_sha256: "55".repeat(32),
                dispatch_artifact_sha256: "66".repeat(32),
            },
            child: PrivateChildCommand {
                executable: PrivateFileIdentity {
                    path: "/private/game".to_owned(),
                    bytes: 999,
                    sha256: "77".repeat(32),
                },
                working_directory: "/private/run".to_owned(),
                argv: vec!["--headless".to_owned(), "value".to_owned()],
                environment: vec![
                    PrivateEnvironmentEntry {
                        name: "A_FIXED".to_owned(),
                        value: "1".to_owned(),
                    },
                    PrivateEnvironmentEntry {
                        name: "Z_FIXED".to_owned(),
                        value: "two".to_owned(),
                    },
                ],
            },
            contract_sha256: String::new(),
        };
        assert_eq!(
            contract.recompute_contract_sha256().unwrap(),
            "e4ca4cf7a3a6beaf88515ffc04d235c74fabf63f8d99cec5f20cb359a13712b3"
        );
    }

    #[test]
    fn file_identity_drift_and_contract_digest_tamper_fail_closed() {
        let directory = TestDirectory::new();
        let (mut contract, _) = fixture_contract(&directory.0);
        contract.contract_sha256 = "00".repeat(32);
        assert!(contract
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("mismatch"));

        let (contract, _) = fixture_contract(&directory.0.join("other"));
        fs::write(&contract.input.path, b"mutated synthetic input").unwrap();
        assert!(contract
            .verify_bound_files()
            .unwrap_err()
            .to_string()
            .contains("identity drift"));
    }
}
