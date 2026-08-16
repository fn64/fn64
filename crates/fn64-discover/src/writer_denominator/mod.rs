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
    CpuCopyStoreOrDecompression(CpuCopyStoreOrDecompressionReceiptV2),
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

/// Reserved completion shape for CPU copies/stores/decompression. No one
/// evaluation can complete this class: effect/closure authority establishes
/// that evaluation is admissible, block harvest binds the exact evaluated
/// image and outputs, and class aggregation proves that the evaluated set is
/// complete for the program model. Production constructors remain absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct CpuCopyStoreOrDecompressionReceiptV2 {
    effect_closure_certificate: OpaqueValidatorReceiptV2,
    evaluated_image_block_harvest: OpaqueValidatorReceiptV2,
    class_completeness_aggregation: OpaqueValidatorReceiptV2,
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
            ValidatedWriterClassReceiptV2::CpuCopyStoreOrDecompression(
                CpuCopyStoreOrDecompressionReceiptV2 {
                    effect_closure_certificate: receipt(),
                    evaluated_image_block_harvest: receipt(),
                    class_completeness_aggregation: receipt(),
                },
            )
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
pub use fn64_cpu_runtime::WriterChannel as WriterChannelV2;

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
mod tests;
