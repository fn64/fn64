#![allow(clippy::module_inception)]
use super::*;

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
    pub(super) contract: PrivateReleaseRunContract,
}

impl VerifiedPrivateReleaseRunContract {
    pub(super) fn contract(&self) -> &PrivateReleaseRunContract {
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
    pub(super) contract: PrivateReleaseRunContract,
    pub(super) output_directory: PathBuf,
    pub(super) receipt: PrivateReleaseSeriesReceipt,
    pub(super) runner_executable: PathBuf,
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

    pub(super) fn verify_bound_files(&self) -> Result<(), PrivateReleaseSeriesError> {
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
