//! Fail-closed V2 writer-channel denominator and frontier-class matrix.
//!
//! V1's source-frontier receipt remains an inventory.  This module adds the
//! fixed semantic mutation-channel denominator separately from V1's 14
//! diagnostic classes. Aliases, visibility state, destinations, and PI proof
//! gaps are frontier axes, not byte-producing mechanisms. Production callers
//! can construct only `Open` rows in either structure. A completion transition
//! exists only where this module consumes validator-owned move-only authority;
//! copied evidence never supplies a constructor.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt;

use crate::source_closure::OpenWriterClass;

pub const EXECUTABLE_WRITER_FRONTIER_MATRIX_SCHEMA_V2: &str =
    "fn64.executable-writer-frontier-matrix.v2";

/// The exact V2 frontier-class matrix, in canonical wire order.
pub const WRITER_CLASSES_V2: [OpenWriterClass; 14] = [
    OpenWriterClass::IndirectPiEpiCall,
    OpenWriterClass::UnrecognizedRawPiAddressConstruction,
    OpenWriterClass::CpuCopyStoreOrDecompression,
    OpenWriterClass::SpDmaToCpuExecutable,
    OpenWriterClass::SiDmaToCpuExecutable,
    OpenWriterClass::RdpWriteToCpuExecutable,
    OpenWriterClass::Kseg1OrTlbExecutableAlias,
    OpenWriterClass::MutableDmaDescriptorOutsideSlice,
    OpenWriterClass::UnadmittedExceptionOrBevVector,
    OpenWriterClass::CrossBankRawPiCaller,
    OpenWriterClass::HostAbiExecutableWrite,
    OpenWriterClass::InstructionCacheState,
    OpenWriterClass::ExtendedAddressAlias,
    OpenWriterClass::DirectDmaHandleMappingOrCompletion,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterClassBlockerCodeV2 {
    ValidatorUnavailable,
    CoverageOpen,
    FindingOpen,
    EvidenceInvalid,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriterClassBlockerV2 {
    pub code: WriterClassBlockerCodeV2,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWriterClassInputV2 {
    pub class: OpenWriterClass,
    pub blockers: Vec<WriterClassBlockerV2>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriterFrontierMatrixInputV2 {
    pub producer: String,
    /// SHA-256 of the exact program/runtime model the future validators audit.
    pub program_model_sha256: String,
    pub classes: Vec<OpenWriterClassInputV2>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[allow(dead_code)] // Completion stays unreachable until class validators own receipts.
enum WriterClassStateV2 {
    Open {
        blockers: Vec<WriterClassBlockerV2>,
    },
    // There is deliberately no production constructor for this receipt.  A
    // future validator must own construction and validation of its evidence.
    Complete {
        receipt: ValidatedWriterClassReceiptV2,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "validator", content = "receipt", rename_all = "snake_case")]
#[allow(dead_code)] // Each variant is reserved for its future validating module.
enum ValidatedWriterClassReceiptV2 {
    IndirectPiEpiCall(OpaqueValidatorReceiptV2),
    UnrecognizedRawPiAddressConstruction(OpaqueValidatorReceiptV2),
    CpuCopyStoreOrDecompression(OpaqueValidatorReceiptV2),
    SpDmaToCpuExecutable(OpaqueValidatorReceiptV2),
    SiDmaToCpuExecutable(OpaqueValidatorReceiptV2),
    RdpWriteToCpuExecutable(OpaqueValidatorReceiptV2),
    Kseg1OrTlbExecutableAlias(OpaqueValidatorReceiptV2),
    MutableDmaDescriptorOutsideSlice(OpaqueValidatorReceiptV2),
    UnadmittedExceptionOrBevVector(OpaqueValidatorReceiptV2),
    CrossBankRawPiCaller(OpaqueValidatorReceiptV2),
    HostAbiExecutableWrite(OpaqueValidatorReceiptV2),
    InstructionCacheState(OpaqueValidatorReceiptV2),
    ExtendedAddressAlias(OpaqueValidatorReceiptV2),
    DirectDmaHandleMappingOrCompletion(OpaqueValidatorReceiptV2),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct OpaqueValidatorReceiptV2 {
    validator_schema: String,
    evidence_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct WriterClassEvidenceV2 {
    class: OpenWriterClass,
    #[serde(flatten)]
    state: WriterClassStateV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriterFrontierMatrixV2 {
    schema: String,
    producer: String,
    program_model_sha256: String,
    classes: Vec<WriterClassEvidenceV2>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriterFrontierMatrixErrorV2 {
    EmptyProducer,
    InvalidProgramModelSha256,
    MissingClass { class: OpenWriterClass },
    DuplicateClass { class: OpenWriterClass },
    UnexpectedClass { class: OpenWriterClass },
    EmptyBlockers { class: OpenWriterClass },
    EmptyBlockerEvidence { class: OpenWriterClass },
    CanonicalJson(String),
}

impl fmt::Display for WriterFrontierMatrixErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid executable-writer frontier matrix: {self:?}"
        )
    }
}

impl std::error::Error for WriterFrontierMatrixErrorV2 {}

impl WriterFrontierMatrixV2 {
    /// Construct the current fail-closed denominator.
    ///
    /// Every class must occur exactly once and must carry at least one named
    /// blocker.  No public input shape can express `Complete`.
    pub fn new_open(
        mut input: WriterFrontierMatrixInputV2,
    ) -> Result<Self, WriterFrontierMatrixErrorV2> {
        if input.producer.trim().is_empty() {
            return Err(WriterFrontierMatrixErrorV2::EmptyProducer);
        }
        if !is_sha256(&input.program_model_sha256) {
            return Err(WriterFrontierMatrixErrorV2::InvalidProgramModelSha256);
        }

        input.classes.sort_by_key(|row| row.class);
        if let Some(row) = input
            .classes
            .iter()
            .find(|row| !WRITER_CLASSES_V2.contains(&row.class))
        {
            return Err(WriterFrontierMatrixErrorV2::UnexpectedClass { class: row.class });
        }
        for pair in input.classes.windows(2) {
            if pair[0].class == pair[1].class {
                return Err(WriterFrontierMatrixErrorV2::DuplicateClass {
                    class: pair[0].class,
                });
            }
        }
        for class in WRITER_CLASSES_V2 {
            if !input.classes.iter().any(|row| row.class == class) {
                return Err(WriterFrontierMatrixErrorV2::MissingClass { class });
            }
        }

        let mut classes = Vec::with_capacity(WRITER_CLASSES_V2.len());
        for mut row in input.classes {
            if row.blockers.is_empty() {
                return Err(WriterFrontierMatrixErrorV2::EmptyBlockers { class: row.class });
            }
            if row
                .blockers
                .iter()
                .any(|blocker| blocker.evidence.trim().is_empty())
            {
                return Err(WriterFrontierMatrixErrorV2::EmptyBlockerEvidence { class: row.class });
            }
            row.blockers.sort_unstable();
            row.blockers.dedup();
            classes.push(WriterClassEvidenceV2 {
                class: row.class,
                state: WriterClassStateV2::Open {
                    blockers: row.blockers,
                },
            });
        }

        Ok(Self {
            schema: EXECUTABLE_WRITER_FRONTIER_MATRIX_SCHEMA_V2.to_string(),
            producer: input.producer,
            program_model_sha256: input.program_model_sha256,
            classes,
        })
    }

    /// Classes which remain open, derived from the validated fixed denominator.
    pub fn open_classes(&self) -> Vec<OpenWriterClass> {
        self.classes
            .iter()
            .filter_map(|row| {
                matches!(&row.state, WriterClassStateV2::Open { .. }).then_some(row.class)
            })
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.open_classes().is_empty()
    }

    pub fn canonical_json_bytes(&self) -> Result<Vec<u8>, WriterFrontierMatrixErrorV2> {
        serde_json::to_vec(self)
            .map_err(|error| WriterFrontierMatrixErrorV2::CanonicalJson(error.to_string()))
    }

    pub fn canonical_sha256(&self) -> Result<String, WriterFrontierMatrixErrorV2> {
        Ok(format!(
            "{:x}",
            Sha256::digest(self.canonical_json_bytes()?)
        ))
    }

    #[cfg(test)]
    fn complete_for_test(producer: &str, program_model_sha256: &str) -> Self {
        Self {
            schema: EXECUTABLE_WRITER_FRONTIER_MATRIX_SCHEMA_V2.to_string(),
            producer: producer.to_string(),
            program_model_sha256: program_model_sha256.to_string(),
            classes: WRITER_CLASSES_V2
                .into_iter()
                .map(|class| WriterClassEvidenceV2 {
                    class,
                    state: WriterClassStateV2::Complete {
                        receipt: validated_receipt_for_test(class, program_model_sha256),
                    },
                })
                .collect(),
        }
    }
}

#[cfg(test)]
fn validated_receipt_for_test(
    class: OpenWriterClass,
    evidence_sha256: &str,
) -> ValidatedWriterClassReceiptV2 {
    let receipt = || OpaqueValidatorReceiptV2 {
        validator_schema: "fn64.synthetic-writer-validator.v1".to_string(),
        evidence_sha256: evidence_sha256.to_string(),
    };
    match class {
        OpenWriterClass::IndirectPiEpiCall => {
            ValidatedWriterClassReceiptV2::IndirectPiEpiCall(receipt())
        }
        OpenWriterClass::UnrecognizedRawPiAddressConstruction => {
            ValidatedWriterClassReceiptV2::UnrecognizedRawPiAddressConstruction(receipt())
        }
        OpenWriterClass::CpuCopyStoreOrDecompression => {
            ValidatedWriterClassReceiptV2::CpuCopyStoreOrDecompression(receipt())
        }
        OpenWriterClass::SpDmaToCpuExecutable => {
            ValidatedWriterClassReceiptV2::SpDmaToCpuExecutable(receipt())
        }
        OpenWriterClass::SiDmaToCpuExecutable => {
            ValidatedWriterClassReceiptV2::SiDmaToCpuExecutable(receipt())
        }
        OpenWriterClass::RdpWriteToCpuExecutable => {
            ValidatedWriterClassReceiptV2::RdpWriteToCpuExecutable(receipt())
        }
        OpenWriterClass::Kseg1OrTlbExecutableAlias => {
            ValidatedWriterClassReceiptV2::Kseg1OrTlbExecutableAlias(receipt())
        }
        OpenWriterClass::MutableDmaDescriptorOutsideSlice => {
            ValidatedWriterClassReceiptV2::MutableDmaDescriptorOutsideSlice(receipt())
        }
        OpenWriterClass::UnadmittedExceptionOrBevVector => {
            ValidatedWriterClassReceiptV2::UnadmittedExceptionOrBevVector(receipt())
        }
        OpenWriterClass::CrossBankRawPiCaller => {
            ValidatedWriterClassReceiptV2::CrossBankRawPiCaller(receipt())
        }
        OpenWriterClass::HostAbiExecutableWrite => {
            ValidatedWriterClassReceiptV2::HostAbiExecutableWrite(receipt())
        }
        OpenWriterClass::InstructionCacheState => {
            ValidatedWriterClassReceiptV2::InstructionCacheState(receipt())
        }
        OpenWriterClass::ExtendedAddressAlias => {
            ValidatedWriterClassReceiptV2::ExtendedAddressAlias(receipt())
        }
        OpenWriterClass::DirectDmaHandleMappingOrCompletion => {
            ValidatedWriterClassReceiptV2::DirectDmaHandleMappingOrCompletion(receipt())
        }
    }
}

pub const EXECUTABLE_WRITER_CHANNEL_DENOMINATOR_SCHEMA_V2: &str =
    "fn64.executable-writer-channel-denominator.v2";

/// The byte-producing mechanisms admitted by the current runtime architecture.
///
/// Address aliases, cache state, exception destinations, and PI analysis gaps
/// belong to the separate frontier matrix above.
pub use fn64_recomp_rs::WriterChannel as WriterChannelV2;

pub const WRITER_CHANNELS_V2: [WriterChannelV2; 8] = [
    WriterChannelV2::CpuInstructionStore,
    WriterChannelV2::PiDma,
    WriterChannelV2::SiDma,
    WriterChannelV2::SpDma,
    WriterChannelV2::RspExecutionOrHleWriteback,
    WriterChannelV2::RdpRenderer,
    WriterChannelV2::HostAbi,
    WriterChannelV2::BootstrapOrImport,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterChannelBlockerCodeV2 {
    ValidatorUnavailable,
    MutableApiEscape,
    UninstrumentedPath,
    CoverageOpen,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriterChannelBlockerV2 {
    pub code: WriterChannelBlockerCodeV2,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWriterChannelInputV2 {
    pub channel: WriterChannelV2,
    pub blockers: Vec<WriterChannelBlockerV2>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriterChannelDenominatorInputV2 {
    pub producer: String,
    pub program_model_sha256: String,
    pub channels: Vec<OpenWriterChannelInputV2>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[allow(dead_code)] // Remaining variants are reserved for their validator authorities.
enum WriterChannelStateV2 {
    Open {
        blockers: Vec<WriterChannelBlockerV2>,
    },
    Complete {
        receipt: ValidatedWriterChannelReceiptV2,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "validator", content = "receipt", rename_all = "snake_case")]
#[allow(dead_code)] // Remaining variants are reserved for their validating modules.
enum ValidatedWriterChannelReceiptV2 {
    CpuInstructionStore(OpaqueValidatorReceiptV2),
    PiDma(OpaqueValidatorReceiptV2),
    SiDma(ValidatedSiDmaWriterChannelReceiptV2),
    SpDma(ValidatedSpDmaWriterChannelReceiptV2),
    RspExecutionOrHleWriteback(OpaqueValidatorReceiptV2),
    RdpRenderer(OpaqueValidatorReceiptV2),
    HostAbi(OpaqueValidatorReceiptV2),
    WriterAuditBundle(ValidatedWriterAuditBundleReceiptV2),
}

/// Denominator-local projection minted only while consuming the selected-build
/// writer-audit bundle. It records the bundle and the represented channel's
/// series authority, but cannot recreate either capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ValidatedWriterAuditBundleReceiptV2 {
    bundle_validator_schema: String,
    bundle_authority_sha256: String,
    channel_series_authority_sha256: String,
}

/// Denominator-local projection minted only while consuming the boot-harness
/// series capability. The serializable fields retain what completed this row;
/// they are not accepted by any completion API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ValidatedSiDmaWriterChannelReceiptV2 {
    validator_schema: String,
    series_authority_sha256: String,
}

/// Denominator-local projection minted only while consuming the boot-harness
/// SP series capability. The retained fields cannot recreate that capability
/// or complete another denominator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ValidatedSpDmaWriterChannelReceiptV2 {
    validator_schema: String,
    series_authority_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct WriterChannelEvidenceV2 {
    channel: WriterChannelV2,
    #[serde(flatten)]
    state: WriterChannelStateV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriterChannelDenominatorV2 {
    schema: String,
    producer: String,
    program_model_sha256: String,
    channels: Vec<WriterChannelEvidenceV2>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriterChannelDenominatorErrorV2 {
    EmptyProducer,
    InvalidProgramModelSha256,
    MissingChannel {
        channel: WriterChannelV2,
    },
    DuplicateChannel {
        channel: WriterChannelV2,
    },
    EmptyBlockers {
        channel: WriterChannelV2,
    },
    EmptyBlockerEvidence {
        channel: WriterChannelV2,
    },
    SiProgramModelMismatch {
        expected: String,
        actual: String,
    },
    SiRowAlreadyComplete,
    InvalidSiAuthority,
    SpProgramModelMismatch {
        expected: String,
        actual: String,
    },
    SpRowAlreadyComplete,
    InvalidSpAuthority,
    InvalidWriterAuditBundle,
    WriterAuditBundleProgramModelMismatch {
        channel: WriterChannelV2,
        expected: String,
        actual: String,
    },
    WriterAuditBundleRowAlreadyComplete {
        channel: WriterChannelV2,
    },
    CanonicalJson(String),
}

impl fmt::Display for WriterChannelDenominatorErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid executable-writer channel denominator: {self:?}"
        )
    }
}

impl std::error::Error for WriterChannelDenominatorErrorV2 {}

impl WriterChannelDenominatorV2 {
    pub fn new_open(
        mut input: WriterChannelDenominatorInputV2,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        if input.producer.trim().is_empty() {
            return Err(WriterChannelDenominatorErrorV2::EmptyProducer);
        }
        if !is_sha256(&input.program_model_sha256) {
            return Err(WriterChannelDenominatorErrorV2::InvalidProgramModelSha256);
        }
        input.channels.sort_by_key(|row| row.channel);
        for pair in input.channels.windows(2) {
            if pair[0].channel == pair[1].channel {
                return Err(WriterChannelDenominatorErrorV2::DuplicateChannel {
                    channel: pair[0].channel,
                });
            }
        }
        for channel in WRITER_CHANNELS_V2 {
            if !input.channels.iter().any(|row| row.channel == channel) {
                return Err(WriterChannelDenominatorErrorV2::MissingChannel { channel });
            }
        }

        let mut channels = Vec::with_capacity(WRITER_CHANNELS_V2.len());
        for mut row in input.channels {
            if row.blockers.is_empty() {
                return Err(WriterChannelDenominatorErrorV2::EmptyBlockers {
                    channel: row.channel,
                });
            }
            if row
                .blockers
                .iter()
                .any(|blocker| blocker.evidence.trim().is_empty())
            {
                return Err(WriterChannelDenominatorErrorV2::EmptyBlockerEvidence {
                    channel: row.channel,
                });
            }
            row.blockers.sort_unstable();
            row.blockers.dedup();
            channels.push(WriterChannelEvidenceV2 {
                channel: row.channel,
                state: WriterChannelStateV2::Open {
                    blockers: row.blockers,
                },
            });
        }

        Ok(Self {
            schema: EXECUTABLE_WRITER_CHANNEL_DENOMINATOR_SCHEMA_V2.to_string(),
            producer: input.producer,
            program_model_sha256: input.program_model_sha256,
            channels,
        })
    }

    pub fn open_channels(&self) -> Vec<WriterChannelV2> {
        self.channels
            .iter()
            .filter_map(|row| {
                matches!(&row.state, WriterChannelStateV2::Open { .. }).then_some(row.channel)
            })
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.open_channels().is_empty()
    }

    /// Consume verifier-owned selected-build evidence and atomically project
    /// exactly its represented Bootstrap/CPU/HostAbi/PI/RDP/RSP/SI/SP rows into this denominator.
    ///
    /// The bundle is move-only and validates its private selected-build inputs
    /// before this method examines it. All represented rows and model digests
    /// are preflighted before this method mutates a denominator row.
    #[cfg(feature = "writer-runtime-authority")]
    pub fn complete_writer_audit_bundle(
        self,
        bundle: fn64_boot_harness::VerifiedGeneratedRunnerWriterAuditBundleV1,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        let evidence = bundle.evidence();
        let authority =
            WriterAuditBundleCompletionAuthorityV2 {
                evidence_valid: bundle.has_valid_evidence_hash(),
                validator_schema: evidence.schema.to_owned(),
                bundle_authority_sha256: evidence.authority_sha256.clone(),
                completed_channels: evidence.completed_channels,
                completions: [
                    (
                        fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
                        WriterChannelV2::BootstrapOrImport,
                        evidence.bootstrap.as_ref().map(|series| {
                            WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::BootstrapOrImport,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }
                        }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
                        WriterChannelV2::CpuInstructionStore,
                        evidence
                            .cpu
                            .as_ref()
                            .map(|series| WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::CpuInstructionStore,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
                        WriterChannelV2::HostAbi,
                        evidence
                            .host_abi
                            .as_ref()
                            .map(|series| WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::HostAbi,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
                        WriterChannelV2::PiDma,
                        evidence
                            .pi
                            .as_ref()
                            .map(|series| WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::PiDma,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
                        WriterChannelV2::RdpRenderer,
                        evidence.rdp_renderer.as_ref().map(|series| {
                            WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::RdpRenderer,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }
                        }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
                        WriterChannelV2::RspExecutionOrHleWriteback,
                        evidence
                            .rsp
                            .as_ref()
                            .map(|series| WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::RspExecutionOrHleWriteback,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                        WriterChannelV2::SiDma,
                        evidence
                            .si
                            .as_ref()
                            .map(|series| WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::SiDma,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }),
                    ),
                    (
                        fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1,
                        WriterChannelV2::SpDma,
                        evidence
                            .sp
                            .as_ref()
                            .map(|series| WriterAuditBundleRowCompletionV2 {
                                channel: WriterChannelV2::SpDma,
                                program_model_sha256: series.program_model_sha256.clone(),
                                series_authority_sha256: series.authority_sha256.clone(),
                            }),
                    ),
                ]
                .into_iter()
                .filter_map(|(bit, _, completion)| {
                    (evidence.completed_channels & bit != 0)
                        .then_some(completion)
                        .flatten()
                })
                .collect(),
            };
        self.complete_writer_audit_bundle_authority(authority)
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn complete_writer_audit_bundle_authority(
        mut self,
        authority: WriterAuditBundleCompletionAuthorityV2,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        let known_mask = fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1;
        if !authority.evidence_valid
            || authority.validator_schema
                != fn64_boot_harness::VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
            || !is_sha256(&authority.bundle_authority_sha256)
            || authority.completed_channels == 0
            || authority.completed_channels & !known_mask != 0
            || authority.completions.len() != authority.completed_channels.count_ones() as usize
        {
            return Err(WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle);
        }
        for completion in &authority.completions {
            let expected_bit = match completion.channel {
                WriterChannelV2::BootstrapOrImport => {
                    fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
                }
                WriterChannelV2::CpuInstructionStore => {
                    fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                }
                WriterChannelV2::HostAbi => fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
                WriterChannelV2::PiDma => fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
                WriterChannelV2::RdpRenderer => {
                    fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
                }
                WriterChannelV2::RspExecutionOrHleWriteback => {
                    fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1
                }
                WriterChannelV2::SiDma => fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                WriterChannelV2::SpDma => fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1,
            };
            if authority.completed_channels & expected_bit == 0
                || !is_sha256(&completion.program_model_sha256)
                || !is_sha256(&completion.series_authority_sha256)
                || authority
                    .completions
                    .iter()
                    .filter(|other| other.channel == completion.channel)
                    .count()
                    != 1
            {
                return Err(WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle);
            }
        }

        for completion in &authority.completions {
            if self.program_model_sha256 != completion.program_model_sha256 {
                return Err(
                    WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                        channel: completion.channel,
                        expected: self.program_model_sha256,
                        actual: completion.program_model_sha256.clone(),
                    },
                );
            }
            let row = self
                .channels
                .iter()
                .find(|row| row.channel == completion.channel)
                .expect("validated fixed denominator lost writer-audit row");
            if !matches!(&row.state, WriterChannelStateV2::Open { .. }) {
                return Err(
                    WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                        channel: completion.channel,
                    },
                );
            }
        }

        for completion in authority.completions {
            let row = self
                .channels
                .iter_mut()
                .find(|row| row.channel == completion.channel)
                .expect("validated fixed denominator lost writer-audit row");
            row.state = WriterChannelStateV2::Complete {
                receipt: ValidatedWriterChannelReceiptV2::WriterAuditBundle(
                    ValidatedWriterAuditBundleReceiptV2 {
                        bundle_validator_schema: authority.validator_schema.clone(),
                        bundle_authority_sha256: authority.bundle_authority_sha256.clone(),
                        channel_series_authority_sha256: completion.series_authority_sha256,
                    },
                ),
            };
        }
        Ok(self)
    }

    /// Consume the parent-owned exact-ten generated-runner authority for SI.
    ///
    /// Pointer-free runtime reports and copied series evidence are
    /// intentionally inadmissible: only the move-only capability can cross
    /// this API. The retained JSON projects its validated authority digest but
    /// cannot recreate a capability or complete another denominator.
    #[cfg(feature = "writer-runtime-authority")]
    pub fn complete_si(
        self,
        series: fn64_boot_harness::VerifiedGeneratedRunnerSiRuntimeSeriesV1,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        let authority = SiDmaCompletionAuthorityV2 {
            evidence_valid: series.has_valid_evidence_hash(),
            validator_schema: series.evidence().schema.to_owned(),
            program_model_sha256: series.evidence().program_model_sha256.clone(),
            series_authority_sha256: series.evidence().authority_sha256.clone(),
        };
        self.complete_si_authority(authority)
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn complete_si_authority(
        mut self,
        authority: SiDmaCompletionAuthorityV2,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        if !authority.evidence_valid
            || !is_sha256(&authority.series_authority_sha256)
            || authority.validator_schema
                != fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
        {
            return Err(WriterChannelDenominatorErrorV2::InvalidSiAuthority);
        }
        if self.program_model_sha256 != authority.program_model_sha256 {
            return Err(WriterChannelDenominatorErrorV2::SiProgramModelMismatch {
                expected: self.program_model_sha256,
                actual: authority.program_model_sha256,
            });
        }
        let row = self
            .channels
            .iter_mut()
            .find(|row| row.channel == WriterChannelV2::SiDma)
            .expect("validated fixed denominator lost SiDma row");
        if !matches!(&row.state, WriterChannelStateV2::Open { .. }) {
            return Err(WriterChannelDenominatorErrorV2::SiRowAlreadyComplete);
        }
        row.state = WriterChannelStateV2::Complete {
            receipt: ValidatedWriterChannelReceiptV2::SiDma(ValidatedSiDmaWriterChannelReceiptV2 {
                validator_schema: authority.validator_schema,
                series_authority_sha256: authority.series_authority_sha256,
            }),
        };
        Ok(self)
    }

    /// Consume the parent-owned exact-ten generated-runner authority for SP.
    ///
    /// The public SP report, its ABI-local prerequisite, and copied series
    /// evidence are intentionally inadmissible. Only the move-only capability
    /// can cross this API, and its retained projection cannot be replayed.
    #[cfg(feature = "writer-runtime-authority")]
    pub fn complete_sp(
        self,
        series: fn64_boot_harness::VerifiedGeneratedRunnerSpRuntimeSeriesV1,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        let authority = SpDmaCompletionAuthorityV2 {
            evidence_valid: series.has_valid_evidence_hash(),
            validator_schema: series.evidence().schema.to_owned(),
            program_model_sha256: series.evidence().program_model_sha256.clone(),
            series_authority_sha256: series.evidence().authority_sha256.clone(),
        };
        self.complete_sp_authority(authority)
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn complete_sp_authority(
        mut self,
        authority: SpDmaCompletionAuthorityV2,
    ) -> Result<Self, WriterChannelDenominatorErrorV2> {
        if !authority.evidence_valid
            || !is_sha256(&authority.series_authority_sha256)
            || authority.validator_schema
                != fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
        {
            return Err(WriterChannelDenominatorErrorV2::InvalidSpAuthority);
        }
        if self.program_model_sha256 != authority.program_model_sha256 {
            return Err(WriterChannelDenominatorErrorV2::SpProgramModelMismatch {
                expected: self.program_model_sha256,
                actual: authority.program_model_sha256,
            });
        }
        let row = self
            .channels
            .iter_mut()
            .find(|row| row.channel == WriterChannelV2::SpDma)
            .expect("validated fixed denominator lost SpDma row");
        if !matches!(&row.state, WriterChannelStateV2::Open { .. }) {
            return Err(WriterChannelDenominatorErrorV2::SpRowAlreadyComplete);
        }
        row.state = WriterChannelStateV2::Complete {
            receipt: ValidatedWriterChannelReceiptV2::SpDma(ValidatedSpDmaWriterChannelReceiptV2 {
                validator_schema: authority.validator_schema,
                series_authority_sha256: authority.series_authority_sha256,
            }),
        };
        Ok(self)
    }

    pub fn canonical_json_bytes(&self) -> Result<Vec<u8>, WriterChannelDenominatorErrorV2> {
        serde_json::to_vec(self)
            .map_err(|error| WriterChannelDenominatorErrorV2::CanonicalJson(error.to_string()))
    }

    pub fn canonical_sha256(&self) -> Result<String, WriterChannelDenominatorErrorV2> {
        Ok(format!(
            "{:x}",
            Sha256::digest(self.canonical_json_bytes()?)
        ))
    }
}

#[cfg(feature = "writer-runtime-authority")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct SiDmaCompletionAuthorityV2 {
    evidence_valid: bool,
    validator_schema: String,
    program_model_sha256: String,
    series_authority_sha256: String,
}

#[cfg(feature = "writer-runtime-authority")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct SpDmaCompletionAuthorityV2 {
    evidence_valid: bool,
    validator_schema: String,
    program_model_sha256: String,
    series_authority_sha256: String,
}

#[cfg(feature = "writer-runtime-authority")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct WriterAuditBundleCompletionAuthorityV2 {
    evidence_valid: bool,
    validator_schema: String,
    bundle_authority_sha256: String,
    completed_channels: u8,
    completions: Vec<WriterAuditBundleRowCompletionV2>,
}

#[cfg(feature = "writer-runtime-authority")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct WriterAuditBundleRowCompletionV2 {
    channel: WriterChannelV2,
    program_model_sha256: String,
    series_authority_sha256: String,
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[cfg(feature = "writer-runtime-authority")]
    fn si_authority(
        program_model_sha256: &str,
        evidence_valid: bool,
    ) -> SiDmaCompletionAuthorityV2 {
        SiDmaCompletionAuthorityV2 {
            evidence_valid,
            validator_schema: fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
                .to_owned(),
            program_model_sha256: program_model_sha256.to_owned(),
            series_authority_sha256:
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned(),
        }
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn sp_authority(
        program_model_sha256: &str,
        evidence_valid: bool,
    ) -> SpDmaCompletionAuthorityV2 {
        SpDmaCompletionAuthorityV2 {
            evidence_valid,
            validator_schema: fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
                .to_owned(),
            program_model_sha256: program_model_sha256.to_owned(),
            series_authority_sha256:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
        }
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn writer_audit_bundle_authority(
        completed_channels: u8,
        program_model_sha256: &str,
    ) -> WriterAuditBundleCompletionAuthorityV2 {
        let completions = [
            (
                fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
                WriterChannelV2::BootstrapOrImport,
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
                WriterChannelV2::CpuInstructionStore,
                "34567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
                WriterChannelV2::HostAbi,
                "567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
                WriterChannelV2::PiDma,
                "4567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef123",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
                WriterChannelV2::RdpRenderer,
                "67890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12345",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
                WriterChannelV2::RspExecutionOrHleWriteback,
                "7890abcdef1234567890abcdef1234567890abcdef1234567890abcdef123456",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                WriterChannelV2::SiDma,
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1,
                WriterChannelV2::SpDma,
                "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            ),
        ]
        .into_iter()
        .filter_map(|(bit, channel, series_authority_sha256)| {
            (completed_channels & bit != 0).then(|| WriterAuditBundleRowCompletionV2 {
                channel,
                program_model_sha256: program_model_sha256.to_owned(),
                series_authority_sha256: series_authority_sha256.to_owned(),
            })
        })
        .collect();
        WriterAuditBundleCompletionAuthorityV2 {
            evidence_valid: true,
            validator_schema:
                fn64_boot_harness::VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
                    .to_owned(),
            bundle_authority_sha256:
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_owned(),
            completed_channels,
            completions,
        }
    }

    fn input() -> WriterFrontierMatrixInputV2 {
        WriterFrontierMatrixInputV2 {
            producer: "fn64-test".to_string(),
            program_model_sha256: MODEL_SHA.to_string(),
            classes: WRITER_CLASSES_V2
                .into_iter()
                .rev()
                .map(|class| OpenWriterClassInputV2 {
                    class,
                    blockers: vec![WriterClassBlockerV2 {
                        code: WriterClassBlockerCodeV2::ValidatorUnavailable,
                        evidence: format!("validator for {class:?} is not implemented"),
                    }],
                })
                .collect(),
        }
    }

    fn channel_input() -> WriterChannelDenominatorInputV2 {
        WriterChannelDenominatorInputV2 {
            producer: "fn64-test".to_string(),
            program_model_sha256: MODEL_SHA.to_string(),
            channels: WRITER_CHANNELS_V2
                .into_iter()
                .rev()
                .map(|channel| OpenWriterChannelInputV2 {
                    channel,
                    blockers: vec![WriterChannelBlockerV2 {
                        code: WriterChannelBlockerCodeV2::MutableApiEscape,
                        evidence: format!("mutation channel {channel:?} is not sealed"),
                    }],
                })
                .collect(),
        }
    }

    #[test]
    fn production_constructor_requires_and_derives_all_fourteen_open_classes() {
        let receipt = WriterFrontierMatrixV2::new_open(input()).unwrap();
        assert_eq!(receipt.open_classes(), WRITER_CLASSES_V2);
        assert!(!receipt.is_complete());

        let json: serde_json::Value =
            serde_json::from_slice(&receipt.canonical_json_bytes().unwrap()).unwrap();
        assert_eq!(json["schema"], EXECUTABLE_WRITER_FRONTIER_MATRIX_SCHEMA_V2);
        assert_eq!(json["classes"].as_array().unwrap().len(), 14);
        assert!(json["classes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["state"] == "open"));
    }

    #[test]
    fn missing_and_duplicate_classes_fail_closed() {
        let mut missing = input();
        let absent = missing.classes.pop().unwrap().class;
        assert_eq!(
            WriterFrontierMatrixV2::new_open(missing),
            Err(WriterFrontierMatrixErrorV2::MissingClass { class: absent })
        );

        let mut duplicate = input();
        let class = duplicate.classes[0].class;
        duplicate.classes.push(duplicate.classes[0].clone());
        assert_eq!(
            WriterFrontierMatrixV2::new_open(duplicate),
            Err(WriterFrontierMatrixErrorV2::DuplicateClass { class })
        );
    }

    #[test]
    fn open_rows_require_named_evidence() {
        let mut empty = input();
        let class = empty.classes[0].class;
        empty.classes[0].blockers.clear();
        assert_eq!(
            WriterFrontierMatrixV2::new_open(empty),
            Err(WriterFrontierMatrixErrorV2::EmptyBlockers { class })
        );

        let mut unnamed = input();
        let class = unnamed.classes[0].class;
        unnamed.classes[0].blockers[0].evidence = "  ".to_string();
        assert_eq!(
            WriterFrontierMatrixV2::new_open(unnamed),
            Err(WriterFrontierMatrixErrorV2::EmptyBlockerEvidence { class })
        );
    }

    #[test]
    fn canonical_form_ignores_input_and_blocker_order() {
        let mut reordered = input();
        reordered.classes.reverse();
        for row in &mut reordered.classes {
            row.blockers.push(WriterClassBlockerV2 {
                code: WriterClassBlockerCodeV2::CoverageOpen,
                evidence: "coverage denominator remains open".to_string(),
            });
            row.blockers.reverse();
        }
        let mut equivalent = reordered.clone();
        equivalent.classes.reverse();
        for row in &mut equivalent.classes {
            row.blockers.reverse();
        }

        let first = WriterFrontierMatrixV2::new_open(reordered).unwrap();
        let second = WriterFrontierMatrixV2::new_open(equivalent).unwrap();
        assert_eq!(
            first.canonical_json_bytes().unwrap(),
            second.canonical_json_bytes().unwrap()
        );
        assert_eq!(
            first.canonical_sha256().unwrap(),
            second.canonical_sha256().unwrap()
        );
    }

    #[test]
    fn frontier_completion_exists_only_behind_the_private_validator_seam() {
        let receipt = WriterFrontierMatrixV2::complete_for_test("fn64-test", MODEL_SHA);
        assert!(receipt.is_complete());
        assert!(receipt.open_classes().is_empty());
        let json: serde_json::Value =
            serde_json::from_slice(&receipt.canonical_json_bytes().unwrap()).unwrap();
        assert!(json["classes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["state"] == "complete"));
    }

    #[test]
    fn semantic_channels_are_a_distinct_exact_denominator() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(denominator.open_channels(), WRITER_CHANNELS_V2);
        assert!(!denominator.is_complete());
        assert_eq!(WRITER_CHANNELS_V2.len(), 8);
        assert_eq!(WRITER_CLASSES_V2.len(), 14);

        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        assert_eq!(
            json["schema"],
            EXECUTABLE_WRITER_CHANNEL_DENOMINATOR_SCHEMA_V2
        );
        assert_eq!(json["channels"].as_array().unwrap().len(), 8);
        assert!(json.get("classes").is_none());
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_api_requires_the_move_only_bundle_capability() {
        let _: fn(
            WriterChannelDenominatorV2,
            fn64_boot_harness::VerifiedGeneratedRunnerWriterAuditBundleV1,
        ) -> Result<WriterChannelDenominatorV2, WriterChannelDenominatorErrorV2> =
            WriterChannelDenominatorV2::complete_writer_audit_bundle;
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_closes_exactly_its_represented_rows() {
        let completed = fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1;
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                completed, MODEL_SHA,
            ))
            .unwrap();
        assert!(denominator.open_channels().is_empty());
        assert!(denominator.is_complete());
        for channel in [
            WriterChannelV2::BootstrapOrImport,
            WriterChannelV2::CpuInstructionStore,
            WriterChannelV2::HostAbi,
            WriterChannelV2::PiDma,
            WriterChannelV2::RdpRenderer,
            WriterChannelV2::RspExecutionOrHleWriteback,
            WriterChannelV2::SiDma,
            WriterChannelV2::SpDma,
        ] {
            assert!(!denominator.open_channels().contains(&channel));
        }
        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        for channel in [
            "bootstrap_or_import",
            "cpu_instruction_store",
            "host_abi",
            "pi_dma",
            "rdp_renderer",
            "rsp_execution_or_hle_writeback",
            "si_dma",
            "sp_dma",
        ] {
            let row = json["channels"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["channel"] == channel)
                .unwrap();
            assert_eq!(row["receipt"]["validator"], "writer_audit_bundle");
            assert_eq!(
                row["receipt"]["receipt"]["bundle_validator_schema"],
                fn64_boot_harness::VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
            );
            let receipt = row["receipt"]["receipt"].as_object().unwrap();
            assert_eq!(receipt.len(), 3);
            assert!(receipt.contains_key("bundle_authority_sha256"));
            assert!(receipt.contains_key("channel_series_authority_sha256"));
        }
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_can_close_a_strict_subset() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert!(!denominator
            .open_channels()
            .contains(&WriterChannelV2::SiDma));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::BootstrapOrImport));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::SpDma));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::CpuInstructionStore));
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_preflights_models_and_rows_before_any_completion() {
        let completed = fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1;
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    completed,
                    &"30".repeat(32),
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::CpuInstructionStore,
                expected: MODEL_SHA.to_owned(),
                actual: "30".repeat(32),
            }
        );

        let mut pi_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        pi_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::PiDma)
            .unwrap()
            .program_model_sha256 = "31".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(pi_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::PiDma,
                expected: MODEL_SHA.to_owned(),
                actual: "31".repeat(32),
            }
        );

        let mut host_abi_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        host_abi_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::HostAbi)
            .unwrap()
            .program_model_sha256 = "32".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(host_abi_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::HostAbi,
                expected: MODEL_SHA.to_owned(),
                actual: "32".repeat(32),
            }
        );

        let mut rdp_renderer_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        rdp_renderer_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::RdpRenderer)
            .unwrap()
            .program_model_sha256 = "33".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rdp_renderer_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::RdpRenderer,
                expected: MODEL_SHA.to_owned(),
                actual: "33".repeat(32),
            }
        );

        let mut rsp_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        rsp_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::RspExecutionOrHleWriteback)
            .unwrap()
            .program_model_sha256 = "34".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rsp_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::RspExecutionOrHleWriteback,
                expected: MODEL_SHA.to_owned(),
                actual: "34".repeat(32),
            }
        );

        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_si_authority(si_authority(MODEL_SHA, true))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    completed, MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::SiDma,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_malformed_shape_before_row_mutation() {
        let mut malformed = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
            MODEL_SHA,
        );
        malformed.completed_channels |= 0x80;
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(malformed)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut invalid_evidence = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        invalid_evidence.evidence_valid = false;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(invalid_evidence)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_cpu = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_cpu.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_cpu)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_pi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_pi.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_pi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_host_abi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_host_abi.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_host_abi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_rdp_renderer = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_rdp_renderer.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_rdp_renderer)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_rsp = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_rsp.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_rsp)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut pi_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        pi_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(pi_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut host_abi_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        host_abi_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(host_abi_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut rdp_renderer_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        rdp_renderer_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rdp_renderer_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut rsp_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        rsp_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rsp_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_cpu = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_cpu.completions[1] = duplicate_cpu.completions[0].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_cpu)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_pi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_pi.completions[0] = duplicate_pi.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_pi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_host_abi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_host_abi.completions[0] = duplicate_host_abi.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_host_abi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_rdp_renderer = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_rdp_renderer.completions[0] = duplicate_rdp_renderer.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_rdp_renderer)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_rsp = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_rsp.completions[0] = duplicate_rsp.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_rsp)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_an_already_complete_cpu_row() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::CpuInstructionStore,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_pi_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::PiDma,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_host_abi_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::HostAbi,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_rdp_renderer_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::RdpRenderer,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_rsp_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::RspExecutionOrHleWriteback,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_api_requires_the_move_only_series_capability() {
        let _: fn(
            WriterChannelDenominatorV2,
            fn64_boot_harness::VerifiedGeneratedRunnerSiRuntimeSeriesV1,
        ) -> Result<WriterChannelDenominatorV2, WriterChannelDenominatorErrorV2> =
            WriterChannelDenominatorV2::complete_si;
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_validated_authority_completes_only_its_exact_channel() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_si_authority(si_authority(MODEL_SHA, true))
            .unwrap();

        assert_eq!(denominator.open_channels().len(), 7);
        assert!(!denominator
            .open_channels()
            .contains(&WriterChannelV2::SiDma));
        assert!(!denominator.is_complete());
        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        let si = json["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["channel"] == "si_dma")
            .unwrap();
        assert_eq!(si["state"], "complete");
        assert_eq!(si["receipt"]["validator"], "si_dma");
        assert_eq!(
            si["receipt"]["receipt"]["validator_schema"],
            fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
        );
        assert_eq!(
            si["receipt"]["receipt"]["series_authority_sha256"],
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
        let serialized = String::from_utf8(denominator.canonical_json_bytes().unwrap()).unwrap();
        assert!(!serialized.contains("private_build_inputs"));
        assert!(!serialized.contains("selected_binary"));
        assert!(!serialized.contains("nonce_set"));
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_rejects_invalid_capability_evidence() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_si_authority(si_authority(MODEL_SHA, false))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSiAuthority
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_rejects_a_different_program_model() {
        let actual = "10".repeat(32);
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_si_authority(si_authority(&actual, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SiProgramModelMismatch {
                expected: MODEL_SHA.to_owned(),
                actual,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_rejects_an_already_complete_row() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_si_authority(si_authority(MODEL_SHA, true))
            .unwrap();
        assert_eq!(
            denominator
                .complete_si_authority(si_authority(MODEL_SHA, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SiRowAlreadyComplete
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_api_requires_the_move_only_series_capability() {
        let _: fn(
            WriterChannelDenominatorV2,
            fn64_boot_harness::VerifiedGeneratedRunnerSpRuntimeSeriesV1,
        ) -> Result<WriterChannelDenominatorV2, WriterChannelDenominatorErrorV2> =
            WriterChannelDenominatorV2::complete_sp;
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_validated_authority_completes_only_its_exact_channel() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_sp_authority(sp_authority(MODEL_SHA, true))
            .unwrap();

        assert_eq!(denominator.open_channels().len(), 7);
        assert!(!denominator
            .open_channels()
            .contains(&WriterChannelV2::SpDma));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::SiDma));
        assert!(!denominator.is_complete());
        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        let sp = json["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["channel"] == "sp_dma")
            .unwrap();
        assert_eq!(sp["state"], "complete");
        assert_eq!(sp["receipt"]["validator"], "sp_dma");
        assert_eq!(
            sp["receipt"]["receipt"]["validator_schema"],
            fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
        );
        assert_eq!(
            sp["receipt"]["receipt"]["series_authority_sha256"],
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        let serialized = String::from_utf8(denominator.canonical_json_bytes().unwrap()).unwrap();
        assert!(!serialized.contains("private_build_inputs"));
        assert!(!serialized.contains("selected_binary"));
        assert!(!serialized.contains("nonce_set"));
        assert!(!serialized.contains("sp_transition"));
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_rejects_invalid_capability_evidence() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(sp_authority(MODEL_SHA, false))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSpAuthority
        );

        let mut wrong_schema = sp_authority(MODEL_SHA, true);
        wrong_schema.validator_schema = "fn64.synthetic-sp-series.v1".to_owned();
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator.complete_sp_authority(wrong_schema).unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSpAuthority
        );

        let mut malformed_digest = sp_authority(MODEL_SHA, true);
        malformed_digest.series_authority_sha256 = "not-a-sha256".to_owned();
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(malformed_digest)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSpAuthority
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_rejects_a_different_program_model() {
        let actual = "20".repeat(32);
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(sp_authority(&actual, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SpProgramModelMismatch {
                expected: MODEL_SHA.to_owned(),
                actual,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_rejects_an_already_complete_row() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_sp_authority(sp_authority(MODEL_SHA, true))
            .unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(sp_authority(MODEL_SHA, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SpRowAlreadyComplete
        );
    }

    #[test]
    fn missing_and_duplicate_semantic_channels_fail_closed() {
        let mut missing = channel_input();
        let absent = missing.channels.pop().unwrap().channel;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(missing),
            Err(WriterChannelDenominatorErrorV2::MissingChannel { channel: absent })
        );

        let mut duplicate = channel_input();
        let channel = duplicate.channels[0].channel;
        duplicate.channels.push(duplicate.channels[0].clone());
        assert_eq!(
            WriterChannelDenominatorV2::new_open(duplicate),
            Err(WriterChannelDenominatorErrorV2::DuplicateChannel { channel })
        );
    }
}
