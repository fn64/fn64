//! Canonical evidence receipt for the executable-image source frontier.
//!
//! This module is deliberately a schema and validator, not a scanner.  Its
//! inputs describe evidence established by a caller; constructing a receipt
//! does not promote bounded observations into an exhaustiveness claim.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt};

use crate::transfer_scan::{TransferScanEvidenceV1, TransferScanV1};

pub const EXECUTABLE_SOURCE_FRONTIER_SCHEMA_V1: &str = "fn64.executable-source-frontier.v1";

/// Every exception destination the current arbitrary-PC CPU model can select.
/// `0x8000_0100` is the VR4300 cache-error vector, which fn64 does not model,
/// and is deliberately absent from this denominator.
pub const MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1: [u32; 6] = [
    0x8000_0000,
    0x8000_0080,
    0x8000_0180,
    0xbfc0_0200,
    0xbfc0_0280,
    0xbfc0_0380,
];

const BEV_EXCEPTION_VECTOR_DESTINATIONS_V1: [u32; 3] = [0xbfc0_0200, 0xbfc0_0280, 0xbfc0_0380];

const STATUS_BEV: u32 = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialBootTvStandardV1 {
    Ntsc,
    Pal,
    Mpal,
}

impl From<fn64_cpu_runtime::boot::BootTvStandard> for InitialBootTvStandardV1 {
    fn from(standard: fn64_cpu_runtime::boot::BootTvStandard) -> Self {
        match standard {
            fn64_cpu_runtime::boot::BootTvStandard::Ntsc => Self::Ntsc,
            fn64_cpu_runtime::boot::BootTvStandard::Pal => Self::Pal,
            fn64_cpu_runtime::boot::BootTvStandard::Mpal => Self::Mpal,
        }
    }
}

/// Exact initial CP0 Status authority supplied to the block-lane thread 0.
///
/// `Missing` is an explicit open frontier, not an absent optional field. A
/// producer which records a context must first validate the context bytes and
/// their ROM binding; this receipt retains the identities needed to audit that
/// decision without treating initial state as proof about later Status writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "authority", rename_all = "snake_case", deny_unknown_fields)]
pub enum InitialCop0StatusAuthorityV1 {
    Missing,
    BootContext {
        boot_context_sha256: String,
        producer: String,
        normalized_rom_sha256: String,
        ipl3_sha256: String,
        destination_code: u8,
        tv_standard: InitialBootTvStandardV1,
        entry_pc: u32,
        cp0_status: u32,
    },
}

impl InitialCop0StatusAuthorityV1 {
    pub fn bev_is_proven_clear(&self) -> bool {
        matches!(
            self,
            Self::BootContext { cp0_status, .. } if cp0_status & STATUS_BEV == 0
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalExecutableImageIdentityV1 {
    pub image_id: String,
    pub lineage: String,
    pub generation: u64,
    pub va_start: u32,
    pub byte_len: u32,
    pub sha256: String,
    /// Reproducible first attempted fetch from this exact captured generation.
    /// Range containment alone is not executable-entry authority.
    pub first_executed_pc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenseGenerationIdentityV1 {
    pub name: String,
    pub bank_id: u64,
    pub source_rom_start: u32,
    pub source_rom_end: u32,
    pub load_start: u32,
    pub load_end: u32,
    pub loaded_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionalCpuWordStoreRequirementV1 {
    SourceStableUntilLoad,
    StoreSiteExecutes,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuWordStoreBlockerV1 {
    AddressOpen,
    AddressSetAmbiguous {
        addresses: Vec<u32>,
    },
    ValueOpen,
    ValueSetAmbiguous {
        values: Vec<u32>,
    },
    ValueNotUnchangedStaticLoad,
    SourceNotAdmitted {
        address: u32,
    },
    SourceValueMismatch {
        address: u32,
        admitted: u32,
        recovered: u32,
    },
    PathDisagreement,
    RevisitWidened,
}

impl From<&crate::resolve::FixedWordStoreBlocker> for CpuWordStoreBlockerV1 {
    fn from(blocker: &crate::resolve::FixedWordStoreBlocker) -> Self {
        use crate::resolve::FixedWordStoreBlocker as Source;
        match blocker {
            Source::AddressOpen => Self::AddressOpen,
            Source::AddressSetAmbiguous { addresses } => Self::AddressSetAmbiguous {
                addresses: addresses.clone(),
            },
            Source::ValueOpen => Self::ValueOpen,
            Source::ValueSetAmbiguous { values } => Self::ValueSetAmbiguous {
                values: values.clone(),
            },
            Source::ValueNotUnchangedStaticLoad => Self::ValueNotUnchangedStaticLoad,
            Source::SourceNotAdmitted { address } => Self::SourceNotAdmitted { address: *address },
            Source::SourceValueMismatch {
                address,
                admitted,
                recovered,
            } => Self::SourceValueMismatch {
                address: *address,
                admitted: *admitted,
                recovered: *recovered,
            },
            Source::PathDisagreement => Self::PathDisagreement,
            Source::RevisitWidened => Self::RevisitWidened,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionalCpuWordStoreV1 {
    pub writer_bank: String,
    pub writer_bank_id: u64,
    pub site_pc: u32,
    pub destination: u32,
    pub value: u32,
    pub source_bank: String,
    pub source_bank_id: u64,
    pub source_address: u32,
    pub source_value: u32,
    pub open_requirements: Vec<ConditionalCpuWordStoreRequirementV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenCpuWordStoreV1 {
    pub writer_bank: String,
    pub writer_bank_id: u64,
    pub site_pc: u32,
    pub blockers: Vec<CpuWordStoreBlockerV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuStoreScanCoverageV1 {
    BoundedReachableCfg,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuStoreScanV1 {
    pub bank: String,
    pub bank_id: u64,
    pub proven_root_count: u32,
    pub reachable_block_count: u32,
    pub conditional_store_count: u32,
    pub open_store_count: u32,
    pub coverage: CpuStoreScanCoverageV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cop0StatusWriteKindV1 {
    Mtc0,
    Dmtc0,
}

impl From<crate::resolve::Cop0StatusWriteKind> for Cop0StatusWriteKindV1 {
    fn from(kind: crate::resolve::Cop0StatusWriteKind) -> Self {
        match kind {
            crate::resolve::Cop0StatusWriteKind::Mtc0 => Self::Mtc0,
            crate::resolve::Cop0StatusWriteKind::Dmtc0 => Self::Dmtc0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cop0StatusWriteSiteV1 {
    pub site_pc: u32,
    pub instruction_word: u32,
    pub source_register: u8,
    pub kind: Cop0StatusWriteKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cop0StatusValueBlockerV1 {
    NoReachableObservation,
    ValueOpen,
    RevisitWidened,
    ValueSetOverflow { observed: u32 },
    MutableStaticMemorySource { addresses: Vec<u32> },
    Dmtc0Unsupported,
}

impl From<&crate::resolve::Cop0StatusValueBlocker> for Cop0StatusValueBlockerV1 {
    fn from(blocker: &crate::resolve::Cop0StatusValueBlocker) -> Self {
        match blocker {
            crate::resolve::Cop0StatusValueBlocker::NoReachableObservation => {
                Self::NoReachableObservation
            }
            crate::resolve::Cop0StatusValueBlocker::ValueOpen => Self::ValueOpen,
            crate::resolve::Cop0StatusValueBlocker::RevisitWidened => Self::RevisitWidened,
            crate::resolve::Cop0StatusValueBlocker::ValueSetOverflow { observed } => {
                Self::ValueSetOverflow {
                    observed: *observed,
                }
            }
            crate::resolve::Cop0StatusValueBlocker::MutableStaticMemorySource { addresses } => {
                Self::MutableStaticMemorySource {
                    addresses: addresses.clone(),
                }
            }
            crate::resolve::Cop0StatusValueBlocker::Dmtc0Unsupported => Self::Dmtc0Unsupported,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cop0StatusValueProofV1 {
    pub site_pc: u32,
    pub values: Vec<u32>,
    pub known_zero: u32,
    pub known_one: u32,
    pub blockers: Vec<Cop0StatusValueBlockerV1>,
}

impl From<&crate::resolve::Cop0StatusValueProof> for Cop0StatusValueProofV1 {
    fn from(proof: &crate::resolve::Cop0StatusValueProof) -> Self {
        Self {
            site_pc: proof.site_pc,
            values: proof.values.clone(),
            known_zero: proof.known_zero,
            known_one: proof.known_one,
            blockers: proof.blockers.iter().map(Into::into).collect(),
        }
    }
}

impl Cop0StatusValueProofV1 {
    /// Evaluate this row's BEV bit fact. Receipt-level authority still
    /// requires `ExecutableSourceFrontierV1::new` to validate the complete
    /// Status-write denominator and every other writer class.
    pub fn proves_bev_clear(&self) -> bool {
        let exact_values_clear = self.blockers.is_empty()
            && !self.values.is_empty()
            && self.values.iter().all(|value| value & STATUS_BEV == 0);
        let partial_bit_blockers_are_safe = self.blockers.iter().all(|blocker| {
            matches!(
                blocker,
                Cop0StatusValueBlockerV1::ValueOpen
                    | Cop0StatusValueBlockerV1::MutableStaticMemorySource { .. }
            )
        });
        exact_values_clear || (self.known_zero & STATUS_BEV != 0 && partial_bit_blockers_are_safe)
    }
}

impl From<&crate::resolve::Cop0StatusWriteSite> for Cop0StatusWriteSiteV1 {
    fn from(site: &crate::resolve::Cop0StatusWriteSite) -> Self {
        Self {
            site_pc: site.site_pc,
            instruction_word: site.instruction_word,
            source_register: site.source_register,
            kind: site.kind.into(),
        }
    }
}

/// Exhaustive raw-decode and bounded-CFG classification of direct COP0 Status
/// writes in one dense generation. This inventory is a prerequisite for a BEV
/// state proof, not such a proof by itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cop0StatusScanV1 {
    pub bank: String,
    pub bank_id: u64,
    pub aligned_word_count: u32,
    pub proven_code_writes: Vec<Cop0StatusWriteSiteV1>,
    pub proven_data_words: Vec<Cop0StatusWriteSiteV1>,
    pub unclassified_writes: Vec<Cop0StatusWriteSiteV1>,
    pub proven_code_value_proofs: Vec<Cop0StatusValueProofV1>,
    pub open_indirect_sites: Vec<u32>,
}

/// The same exhaustive raw Status-write inventory for one captured executable
/// image outside the dense ROM generations. The capture identity and exact
/// range bind the scan to bytes already admitted by `external_images`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalCop0StatusScanV1 {
    pub image_id: String,
    pub generation: u64,
    pub va_start: u32,
    pub byte_len: u32,
    pub sha256: String,
    pub first_executed_pc: u32,
    pub aligned_word_count: u32,
    pub proven_code_writes: Vec<Cop0StatusWriteSiteV1>,
    pub proven_data_words: Vec<Cop0StatusWriteSiteV1>,
    pub unclassified_writes: Vec<Cop0StatusWriteSiteV1>,
    pub proven_code_value_proofs: Vec<Cop0StatusValueProofV1>,
    pub open_indirect_sites: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionVectorExactCodeOwnerV1 {
    pub image_id: String,
    pub lineage: String,
    pub generation: u64,
    pub va_start: u32,
    pub byte_len: u32,
    pub sha256: String,
    pub first_executed_pc: u32,
}

impl From<&ExternalExecutableImageIdentityV1> for ExceptionVectorExactCodeOwnerV1 {
    fn from(image: &ExternalExecutableImageIdentityV1) -> Self {
        Self {
            image_id: image.image_id.clone(),
            lineage: image.lineage.clone(),
            generation: image.generation,
            va_start: image.va_start,
            byte_len: image.byte_len,
            sha256: image.sha256.clone(),
            first_executed_pc: image.first_executed_pc,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineCheckedUnreachabilityV1 {
    /// Schema of the independently validated state/reachability receipt.
    pub proof_schema: String,
    /// Canonical SHA-256 of that receipt.
    pub proof_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionVectorDispositionV1 {
    ExactCodeOwner(ExceptionVectorExactCodeOwnerV1),
    MachineCheckedUnreachability(MachineCheckedUnreachabilityV1),
    /// Valid only for the three bootstrap vectors and only when this receipt's
    /// in-process validator closes every admitted Status source and executable
    /// source/transfer frontier needed by the induction.
    BevClearInvariant,
    Open {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeledExceptionVectorV1 {
    pub destination: u32,
    pub disposition: ExceptionVectorDispositionV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCurrentStatusEffectV1 {
    PreservesBev,
    CBridgeRuntimeEnforcedPreservesBev,
    CBridgeCopyBackUnclassified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostBindingSymbolV1 {
    OsCreateMesgQueue,
    OsCreateThread,
    OsEPiStartDma,
    OsGetThreadPri,
    OsRecvMesg,
    OsSendMesg,
    OsSetEventMesg,
    OsSiDeviceBusy,
    OsSetThreadPri,
    OsSetTimer,
    OsSpTaskLoad,
    OsSpTaskStartGo,
    OsSpTaskYield,
    OsSpTaskYielded,
    OsStartThread,
    OsDriveRomInit,
    OsEPiWriteIo,
    OsEPiReadIo,
    OsFlashInit,
    OsFlashSectorErase,
    OsFlashReadArray,
}

impl From<crate::host_bindings::HostBindingSymbol> for HostBindingSymbolV1 {
    fn from(symbol: crate::host_bindings::HostBindingSymbol) -> Self {
        use crate::host_bindings::HostBindingSymbol as Source;
        match symbol {
            Source::OsCreateMesgQueue => Self::OsCreateMesgQueue,
            Source::OsCreateThread => Self::OsCreateThread,
            Source::OsDriveRomInit => Self::OsDriveRomInit,
            Source::OsEPiStartDma => Self::OsEPiStartDma,
            Source::OsGetThreadPri => Self::OsGetThreadPri,
            Source::OsRecvMesg => Self::OsRecvMesg,
            Source::OsSendMesg => Self::OsSendMesg,
            Source::OsSetEventMesg => Self::OsSetEventMesg,
            Source::OsSiDeviceBusy => Self::OsSiDeviceBusy,
            Source::OsSetThreadPri => Self::OsSetThreadPri,
            Source::OsSetTimer => Self::OsSetTimer,
            Source::OsSpTaskLoad => Self::OsSpTaskLoad,
            Source::OsSpTaskStartGo => Self::OsSpTaskStartGo,
            Source::OsSpTaskYield => Self::OsSpTaskYield,
            Source::OsSpTaskYielded => Self::OsSpTaskYielded,
            Source::OsStartThread => Self::OsStartThread,
            Source::OsEPiWriteIo => Self::OsEPiWriteIo,
            Source::OsEPiReadIo => Self::OsEPiReadIo,
            Source::OsFlashInit => Self::OsFlashInit,
            Source::OsFlashSectorErase => Self::OsFlashSectorErase,
            Source::OsFlashReadArray => Self::OsFlashReadArray,
        }
    }
}

impl HostBindingSymbolV1 {
    fn current_status_effect(self) -> HostCurrentStatusEffectV1 {
        HostCurrentStatusEffectV1::CBridgeRuntimeEnforcedPreservesBev
    }

    fn spawned_status_effect(self) -> HostSpawnedStatusEffectV1 {
        if self == Self::OsCreateThread {
            HostSpawnedStatusEffectV1::GeneratedSavedSrPostEretClearsBev
        } else {
            HostSpawnedStatusEffectV1::None
        }
    }
}

impl From<crate::host_bindings::HostCurrentStatusEffect> for HostCurrentStatusEffectV1 {
    fn from(effect: crate::host_bindings::HostCurrentStatusEffect) -> Self {
        match effect {
            crate::host_bindings::HostCurrentStatusEffect::CBridgeRuntimeEnforcedPreservesBev => {
                Self::CBridgeRuntimeEnforcedPreservesBev
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSpawnedStatusEffectV1 {
    None,
    /// Retained only so historical v1 receipts fail semantic revalidation
    /// instead of failing to deserialize.
    InheritsCallerClearingFr,
    GeneratedSavedSrPostEretClearsBev,
}

impl From<crate::host_bindings::HostSpawnedStatusEffect> for HostSpawnedStatusEffectV1 {
    fn from(effect: crate::host_bindings::HostSpawnedStatusEffect) -> Self {
        match effect {
            crate::host_bindings::HostSpawnedStatusEffect::None => Self::None,
            crate::host_bindings::HostSpawnedStatusEffect::GeneratedSavedSrPostEretClearsBev => {
                Self::GeneratedSavedSrPostEretClearsBev
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostBindingV1 {
    pub bank: String,
    pub guest_vram: u32,
    pub symbol: HostBindingSymbolV1,
    pub current_status_effect: HostCurrentStatusEffectV1,
    pub spawned_status_effect: HostSpawnedStatusEffectV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSiteDispositionV1 {
    ReachableInstruction,
    ProvenData,
    Unclassified,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSiteV1 {
    pub bank: String,
    pub guest_pc: u32,
    pub raw_word: u32,
    pub decoded_op: String,
    pub base_register: u8,
    pub offset: i16,
    pub word_class: String,
    pub disposition: CacheSiteDispositionV1,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ExecutableDestinationOwnerV1 {
    DenseGeneration {
        bank_id: u64,
        va_start: u32,
        va_end: u32,
    },
    ExternalImage {
        image_id: String,
        generation: u64,
        va_start: u32,
        va_end: u32,
    },
    ProvenNonExecutable {
        physical_start: u32,
        physical_end: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectDmaFindingV1 {
    pub caller_bank: String,
    pub caller_pc: u32,
    pub primitive_bank: String,
    pub primitive_pc: u32,
    pub device_start: u32,
    pub device_end: u32,
    pub rdram_start: u32,
    pub rdram_end: u32,
    pub destination_owner: ExecutableDestinationOwnerV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenWriterClass {
    IndirectPiEpiCall,
    UnrecognizedRawPiAddressConstruction,
    CpuCopyStoreOrDecompression,
    SpDmaToCpuExecutable,
    SiDmaToCpuExecutable,
    RdpWriteToCpuExecutable,
    Kseg1OrTlbExecutableAlias,
    MutableDmaDescriptorOutsideSlice,
    UnadmittedExceptionOrBevVector,
    CrossBankRawPiCaller,
    HostAbiExecutableWrite,
    InstructionCacheState,
    ExtendedAddressAlias,
    DirectDmaHandleMappingOrCompletion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectDmaBlockerCodeV1 {
    SourceRangeUnresolved,
    DestinationRangeUnresolved,
    LengthUnresolved,
    ControlFlowUnresolved,
    MutableDescriptor,
    ImageAdmissionMissing,
    PrimitiveUnrecognized,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectDmaBlockerV1 {
    pub caller_bank: String,
    pub caller_pc: Option<u32>,
    pub primitive_bank: String,
    pub code: DirectDmaBlockerCodeV1,
    pub writer_class: OpenWriterClass,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterResolutionV1 {
    AdmittedDenseGeneration,
    AdmittedExternalImage,
    ProvenNonExecutable,
    Bounded,
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPiCallerV1 {
    pub caller_bank: String,
    pub caller_pc: u32,
    pub primitive_pc: u32,
    pub resolution: WriterResolutionV1,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPiPrimitiveV1 {
    pub bank: String,
    pub entry_pc: u32,
    pub symbol: String,
    pub register_site_pcs: Vec<u32>,
    pub callers: Vec<RawPiCallerV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferSummaryV1 {
    pub direct_total: u64,
    pub direct_guest: u64,
    pub direct_host: u64,
    pub direct_open: u64,
    pub indirect_closed: u64,
    pub indirect_bounded: u64,
    pub indirect_open: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferInventoryV1 {
    Complete,
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectDispositionV1 {
    Closed,
    Bounded,
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndirectTransferFrontierV1 {
    pub bank: String,
    pub guest_pc: u32,
    pub transfer_kind: String,
    pub disposition: IndirectDispositionV1,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableSourceFrontierInputV1 {
    pub producer: String,
    pub normalized_rom_sha256: String,
    pub dense_aot_pack_sha256: String,
    pub initial_cop0_status: InitialCop0StatusAuthorityV1,
    pub dense_generations: Vec<DenseGenerationIdentityV1>,
    pub external_images: Vec<ExternalExecutableImageIdentityV1>,
    pub exception_vectors: Vec<ModeledExceptionVectorV1>,
    pub host_bindings: Vec<HostBindingV1>,
    pub cache_sites: Vec<CacheSiteV1>,
    pub direct_dma_findings: Vec<DirectDmaFindingV1>,
    pub direct_dma_blockers: Vec<DirectDmaBlockerV1>,
    pub raw_pi_primitives: Vec<RawPiPrimitiveV1>,
    pub cpu_store_watched_destinations: Vec<u32>,
    pub cpu_store_scans: Vec<CpuStoreScanV1>,
    pub cop0_status_scans: Vec<Cop0StatusScanV1>,
    pub external_cop0_status_scans: Vec<ExternalCop0StatusScanV1>,
    pub conditional_cpu_word_stores: Vec<ConditionalCpuWordStoreV1>,
    pub open_cpu_word_stores: Vec<OpenCpuWordStoreV1>,
    pub transfer_scan: TransferScanV1,
    pub open_writer_classes: Vec<OpenWriterClass>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExecutableSourceFrontierV1 {
    schema: String,
    producer: String,
    normalized_rom_sha256: String,
    dense_aot_pack_sha256: String,
    initial_cop0_status: InitialCop0StatusAuthorityV1,
    dense_generations: Vec<DenseGenerationIdentityV1>,
    external_images: Vec<ExternalExecutableImageIdentityV1>,
    exception_vectors: Vec<ModeledExceptionVectorV1>,
    host_bindings: Vec<HostBindingV1>,
    cache_sites: Vec<CacheSiteV1>,
    direct_dma_findings: Vec<DirectDmaFindingV1>,
    direct_dma_blockers: Vec<DirectDmaBlockerV1>,
    raw_pi_primitives: Vec<RawPiPrimitiveV1>,
    cpu_store_watched_destinations: Vec<u32>,
    cpu_store_scans: Vec<CpuStoreScanV1>,
    cop0_status_scans: Vec<Cop0StatusScanV1>,
    external_cop0_status_scans: Vec<ExternalCop0StatusScanV1>,
    conditional_cpu_word_stores: Vec<ConditionalCpuWordStoreV1>,
    open_cpu_word_stores: Vec<OpenCpuWordStoreV1>,
    transfer_scan: TransferScanEvidenceV1,
    open_writer_classes: Vec<OpenWriterClass>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceFrontierDiagnosticsV1 {
    pub initial_bev_clear: bool,
    pub external_images: usize,
    pub open_exception_vectors: usize,
    pub open_writer_classes: usize,
    pub cache_sites: usize,
    pub unclassified_cache_sites: usize,
    pub direct_dma_blockers: usize,
    pub raw_pi_primitives: usize,
    pub raw_pi_open_callers: usize,
    pub cpu_store_scans: usize,
    pub cop0_status_scans: usize,
    pub external_cop0_status_scans: usize,
    pub cop0_unclassified_writes: usize,
    pub cop0_value_open: usize,
    pub conditional_cpu_word_stores: usize,
    pub open_cpu_word_stores: usize,
    pub transfer_inventory_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceFrontierError {
    EmptyProducer,
    InvalidSha256 {
        field: &'static str,
    },
    InvalidInitialCop0StatusAuthority {
        field: &'static str,
    },
    InvalidDenseGeneration {
        bank: String,
        bank_id: u64,
    },
    AmbiguousDenseGenerationIdentity {
        bank: String,
        bank_id: u64,
    },
    InvalidExternalImageRange {
        image_id: String,
        generation: u64,
    },
    OverlappingExternalImages {
        first_image_id: String,
        first_generation: u64,
        second_image_id: String,
        second_generation: u64,
    },
    InvalidDirectDmaRange {
        caller_pc: u32,
    },
    AmbiguousExternalImageIdentity {
        image_id: String,
        generation: u64,
    },
    MissingModeledExceptionVector {
        destination: u32,
    },
    DuplicateModeledExceptionVector {
        destination: u32,
    },
    UnexpectedModeledExceptionVector {
        destination: u32,
    },
    InvalidExceptionVectorOwner {
        destination: u32,
    },
    InvalidExceptionVectorUnreachability {
        destination: u32,
    },
    InvalidBevClearVectorUnreachability {
        destination: u32,
    },
    InvalidHostBindingCatalog,
    EmptyExceptionVectorOpenReason {
        destination: u32,
    },
    InvalidConditionalCpuWordStore {
        bank: String,
        site_pc: u32,
    },
    InvalidOpenCpuWordStore {
        bank: String,
        site_pc: u32,
    },
    MissingCpuStoreScan {
        bank: String,
        bank_id: u64,
    },
    InvalidCpuStoreScan {
        bank: String,
        bank_id: u64,
    },
    MissingCop0StatusScan {
        bank: String,
        bank_id: u64,
    },
    InvalidCop0StatusScan {
        bank: String,
        bank_id: u64,
    },
    MissingExternalCop0StatusScan {
        image_id: String,
        generation: u64,
    },
    InvalidExternalCop0StatusScan {
        image_id: String,
        generation: u64,
    },
    InvalidTransferInventory,
    CanonicalJson(String),
}

impl fmt::Display for SourceFrontierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid executable-source frontier: {self:?}")
    }
}

impl std::error::Error for SourceFrontierError {}

fn require_sha256(value: &str, field: &'static str) -> Result<(), SourceFrontierError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(SourceFrontierError::InvalidSha256 { field })
    }
}

fn sort_dedup<T: Ord>(values: &mut Vec<T>) {
    values.sort_unstable();
    values.dedup();
}

fn valid_cop0_status_site(site: &Cop0StatusWriteSiteV1) -> bool {
    use fn64_cpu_runtime::decoder::{decode, Instruction};
    matches!(
        (decode(site.instruction_word), site.kind),
        (
            Instruction::Mtc0 { rt, cop0d: 12 },
            Cop0StatusWriteKindV1::Mtc0
        ) if rt == site.source_register
    ) || matches!(
        (decode(site.instruction_word), site.kind),
        (
            Instruction::Dmtc0 { rt, cop0d: 12 },
            Cop0StatusWriteKindV1::Dmtc0
        ) if rt == site.source_register
    )
}

fn normalize_cop0_status_value_proof(proof: &mut Cop0StatusValueProofV1) {
    sort_dedup(&mut proof.values);
    for blocker in &mut proof.blockers {
        if let Cop0StatusValueBlockerV1::MutableStaticMemorySource { addresses } = blocker {
            sort_dedup(addresses);
        }
    }
    sort_dedup(&mut proof.blockers);
}

fn valid_cop0_status_value_proofs(
    sites: &[Cop0StatusWriteSiteV1],
    proofs: &[Cop0StatusValueProofV1],
) -> bool {
    if sites.len() != proofs.len() {
        return false;
    }
    sites.iter().all(|site| {
        let matching = proofs
            .iter()
            .filter(|proof| proof.site_pc == site.site_pc)
            .collect::<Vec<_>>();
        let [proof] = matching.as_slice() else {
            return false;
        };
        if proof.values.len() > 256
            || proof.known_zero & proof.known_one != 0
            || (proof.values.is_empty()
                && proof.known_zero == 0
                && proof.known_one == 0
                && proof.blockers.is_empty())
            || proof.values.iter().any(|value| {
                value & proof.known_zero != 0 || value & proof.known_one != proof.known_one
            })
        {
            return false;
        }
        let unsupported = proof
            .blockers
            .contains(&Cop0StatusValueBlockerV1::Dmtc0Unsupported);
        if unsupported != (site.kind == Cop0StatusWriteKindV1::Dmtc0) {
            return false;
        }
        proof.blockers.iter().all(|blocker| {
            !matches!(
                blocker,
                Cop0StatusValueBlockerV1::ValueSetOverflow { observed } if *observed <= 256
            )
        })
    })
}

fn cop0_status_value_proof_is_bev_clear(proof: &Cop0StatusValueProofV1) -> bool {
    proof.proves_bev_clear()
}

impl ExecutableSourceFrontierV1 {
    pub fn diagnostics(&self) -> SourceFrontierDiagnosticsV1 {
        SourceFrontierDiagnosticsV1 {
            initial_bev_clear: self.initial_cop0_status.bev_is_proven_clear(),
            external_images: self.external_images.len(),
            open_exception_vectors: self
                .exception_vectors
                .iter()
                .filter(|vector| {
                    matches!(
                        vector.disposition,
                        ExceptionVectorDispositionV1::Open { .. }
                    )
                })
                .count(),
            open_writer_classes: self.open_writer_classes.len(),
            cache_sites: self.cache_sites.len(),
            unclassified_cache_sites: self
                .cache_sites
                .iter()
                .filter(|site| matches!(site.disposition, CacheSiteDispositionV1::Unclassified))
                .count(),
            direct_dma_blockers: self.direct_dma_blockers.len(),
            raw_pi_primitives: self.raw_pi_primitives.len(),
            raw_pi_open_callers: self
                .raw_pi_primitives
                .iter()
                .flat_map(|primitive| &primitive.callers)
                .filter(|caller| {
                    matches!(
                        caller.resolution,
                        WriterResolutionV1::Bounded | WriterResolutionV1::Open
                    )
                })
                .count(),
            cpu_store_scans: self.cpu_store_scans.len(),
            cop0_status_scans: self.cop0_status_scans.len(),
            external_cop0_status_scans: self.external_cop0_status_scans.len(),
            cop0_unclassified_writes: self
                .cop0_status_scans
                .iter()
                .map(|scan| scan.unclassified_writes.len())
                .sum::<usize>()
                + self
                    .external_cop0_status_scans
                    .iter()
                    .map(|scan| scan.unclassified_writes.len())
                    .sum::<usize>(),
            cop0_value_open: self
                .cop0_status_scans
                .iter()
                .flat_map(|scan| &scan.proven_code_value_proofs)
                .chain(
                    self.external_cop0_status_scans
                        .iter()
                        .flat_map(|scan| &scan.proven_code_value_proofs),
                )
                .filter(|proof| !cop0_status_value_proof_is_bev_clear(proof))
                .count(),
            conditional_cpu_word_stores: self.conditional_cpu_word_stores.len(),
            open_cpu_word_stores: self.open_cpu_word_stores.len(),
            transfer_inventory_complete: self.transfer_scan.inventory
                == TransferInventoryV1::Complete,
        }
    }

    pub fn new(mut input: ExecutableSourceFrontierInputV1) -> Result<Self, SourceFrontierError> {
        if input.producer.trim().is_empty() {
            return Err(SourceFrontierError::EmptyProducer);
        }
        require_sha256(&input.normalized_rom_sha256, "normalized_rom_sha256")?;
        require_sha256(&input.dense_aot_pack_sha256, "dense_aot_pack_sha256")?;
        if let InitialCop0StatusAuthorityV1::BootContext {
            boot_context_sha256,
            producer,
            normalized_rom_sha256,
            ipl3_sha256,
            entry_pc,
            ..
        } = &input.initial_cop0_status
        {
            require_sha256(
                boot_context_sha256,
                "initial_cop0_status.boot_context_sha256",
            )?;
            require_sha256(
                normalized_rom_sha256,
                "initial_cop0_status.normalized_rom_sha256",
            )?;
            require_sha256(ipl3_sha256, "initial_cop0_status.ipl3_sha256")?;
            if producer.trim().is_empty() {
                return Err(SourceFrontierError::InvalidInitialCop0StatusAuthority {
                    field: "producer",
                });
            }
            if normalized_rom_sha256 != &input.normalized_rom_sha256 {
                return Err(SourceFrontierError::InvalidInitialCop0StatusAuthority {
                    field: "normalized_rom_sha256",
                });
            }
            if entry_pc & 3 != 0 {
                return Err(SourceFrontierError::InvalidInitialCop0StatusAuthority {
                    field: "entry_pc",
                });
            }
        }
        for generation in &input.dense_generations {
            let source_len = generation
                .source_rom_end
                .checked_sub(generation.source_rom_start);
            let load_len = generation.load_end.checked_sub(generation.load_start);
            if generation.name.trim().is_empty()
                || source_len.is_none()
                || source_len == Some(0)
                || source_len != load_len
                || !generation.source_rom_start.is_multiple_of(4)
                || !generation.source_rom_end.is_multiple_of(4)
                || !generation.load_start.is_multiple_of(4)
                || !generation.load_end.is_multiple_of(4)
                || require_sha256(&generation.loaded_sha256, "dense_generations.loaded_sha256")
                    .is_err()
            {
                return Err(SourceFrontierError::InvalidDenseGeneration {
                    bank: generation.name.clone(),
                    bank_id: generation.bank_id,
                });
            }
        }
        for (index, generation) in input.dense_generations.iter().enumerate() {
            if input.dense_generations[..index].iter().any(|known| {
                (known.name == generation.name || known.bank_id == generation.bank_id)
                    && known != generation
            }) {
                return Err(SourceFrontierError::AmbiguousDenseGenerationIdentity {
                    bank: generation.name.clone(),
                    bank_id: generation.bank_id,
                });
            }
        }
        for image in &input.external_images {
            require_sha256(&image.sha256, "external_images.sha256")?;
            if image.byte_len == 0
                || !image.byte_len.is_multiple_of(4)
                || !image.va_start.is_multiple_of(4)
                || image.first_executed_pc < image.va_start
                || image.first_executed_pc >= image.va_start.saturating_add(image.byte_len)
                || !image.first_executed_pc.is_multiple_of(4)
                || image.va_start.checked_add(image.byte_len).is_none()
            {
                return Err(SourceFrontierError::InvalidExternalImageRange {
                    image_id: image.image_id.clone(),
                    generation: image.generation,
                });
            }
        }
        for (index, image) in input.external_images.iter().enumerate() {
            if input.external_images[..index].iter().any(|known| {
                known.image_id == image.image_id
                    && known.generation == image.generation
                    && known != image
            }) {
                return Err(SourceFrontierError::AmbiguousExternalImageIdentity {
                    image_id: image.image_id.clone(),
                    generation: image.generation,
                });
            }
            let image_end = image.va_start + image.byte_len;
            if let Some(known) = input.external_images[..index].iter().find(|known| {
                *known != image
                    && known.va_start < image_end
                    && image.va_start < known.va_start + known.byte_len
            }) {
                return Err(SourceFrontierError::OverlappingExternalImages {
                    first_image_id: known.image_id.clone(),
                    first_generation: known.generation,
                    second_image_id: image.image_id.clone(),
                    second_generation: image.generation,
                });
            }
        }
        for scan in &mut input.external_cop0_status_scans {
            sort_dedup(&mut scan.proven_code_writes);
            sort_dedup(&mut scan.proven_data_words);
            sort_dedup(&mut scan.unclassified_writes);
            for proof in &mut scan.proven_code_value_proofs {
                normalize_cop0_status_value_proof(proof);
            }
            sort_dedup(&mut scan.proven_code_value_proofs);
            sort_dedup(&mut scan.open_indirect_sites);
            let Some(image) = input.external_images.iter().find(|image| {
                image.image_id == scan.image_id
                    && image.generation == scan.generation
                    && image.va_start == scan.va_start
                    && image.byte_len == scan.byte_len
                    && image.sha256 == scan.sha256
                    && image.first_executed_pc == scan.first_executed_pc
            }) else {
                return Err(SourceFrontierError::InvalidExternalCop0StatusScan {
                    image_id: scan.image_id.clone(),
                    generation: scan.generation,
                });
            };
            let image_end = image.va_start + image.byte_len;
            let mut seen_sites = BTreeSet::new();
            let sites_are_valid = scan
                .proven_code_writes
                .iter()
                .chain(&scan.proven_data_words)
                .chain(&scan.unclassified_writes)
                .all(|site| {
                    (image.va_start..image_end).contains(&site.site_pc)
                        && site.site_pc.is_multiple_of(4)
                        && valid_cop0_status_site(site)
                        && seen_sites.insert(site.site_pc)
                });
            let indirect_sites_are_valid = scan.open_indirect_sites.iter().all(|site_pc| {
                (image.va_start..image_end).contains(site_pc) && site_pc.is_multiple_of(4)
            });
            if !(image.va_start..image_end).contains(&scan.first_executed_pc)
                || scan.aligned_word_count != image.byte_len / 4
                || !sites_are_valid
                || !valid_cop0_status_value_proofs(
                    &scan.proven_code_writes,
                    &scan.proven_code_value_proofs,
                )
                || !indirect_sites_are_valid
            {
                return Err(SourceFrontierError::InvalidExternalCop0StatusScan {
                    image_id: scan.image_id.clone(),
                    generation: scan.generation,
                });
            }
        }
        sort_dedup(&mut input.external_cop0_status_scans);
        for image in &input.external_images {
            let matching_scans = input
                .external_cop0_status_scans
                .iter()
                .filter(|scan| {
                    scan.image_id == image.image_id && scan.generation == image.generation
                })
                .count();
            if matching_scans != 1 {
                return Err(SourceFrontierError::MissingExternalCop0StatusScan {
                    image_id: image.image_id.clone(),
                    generation: image.generation,
                });
            }
        }
        sort_dedup(&mut input.host_bindings);
        let expected_host_symbols = crate::host_bindings::WM_BLOCK_RUNTIME_HOST_SYMBOLS
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<HostBindingSymbolV1>>();
        let actual_host_symbols = input
            .host_bindings
            .iter()
            .map(|binding| binding.symbol)
            .collect::<BTreeSet<_>>();
        let unique_host_addresses = input
            .host_bindings
            .iter()
            .map(|binding| binding.guest_vram)
            .collect::<BTreeSet<_>>();
        if input.host_bindings.len() != expected_host_symbols.len()
            || actual_host_symbols != expected_host_symbols
            || unique_host_addresses.len() != input.host_bindings.len()
            || input.host_bindings.iter().any(|binding| {
                binding.bank.trim().is_empty()
                    || !binding.guest_vram.is_multiple_of(4)
                    || binding.current_status_effect != binding.symbol.current_status_effect()
                    || binding.spawned_status_effect != binding.symbol.spawned_status_effect()
            })
        {
            return Err(SourceFrontierError::InvalidHostBindingCatalog);
        }
        input.exception_vectors.sort_unstable();
        for pair in input.exception_vectors.windows(2) {
            if pair[0].destination == pair[1].destination {
                return Err(SourceFrontierError::DuplicateModeledExceptionVector {
                    destination: pair[0].destination,
                });
            }
        }
        for vector in &input.exception_vectors {
            if !MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1.contains(&vector.destination) {
                return Err(SourceFrontierError::UnexpectedModeledExceptionVector {
                    destination: vector.destination,
                });
            }
            match &vector.disposition {
                ExceptionVectorDispositionV1::ExactCodeOwner(owner) => {
                    let owner_end = owner.va_start.checked_add(owner.byte_len);
                    let owns_entry = owner_end.is_some_and(|end| {
                        owner.va_start <= vector.destination
                            && vector
                                .destination
                                .checked_add(4)
                                .is_some_and(|entry_end| entry_end <= end)
                    });
                    let bound_external_image = input.external_images.iter().any(|image| {
                        image.image_id == owner.image_id
                            && image.lineage == owner.lineage
                            && image.generation == owner.generation
                            && image.va_start == owner.va_start
                            && image.byte_len == owner.byte_len
                            && image.sha256 == owner.sha256
                            && image.first_executed_pc == owner.first_executed_pc
                    });
                    if !owns_entry
                        || owner.first_executed_pc != vector.destination
                        || !bound_external_image
                    {
                        return Err(SourceFrontierError::InvalidExceptionVectorOwner {
                            destination: vector.destination,
                        });
                    }
                }
                ExceptionVectorDispositionV1::MachineCheckedUnreachability(proof) => {
                    // No state-reachability receipt schema has an in-process
                    // validator yet. Merely naming a schema and digest cannot
                    // prove its bytes, ROM binding, or claimed invariant, so
                    // this variant remains wire-reserved and fail-closed.
                    let _ = proof;
                    return Err(SourceFrontierError::InvalidExceptionVectorUnreachability {
                        destination: vector.destination,
                    });
                }
                ExceptionVectorDispositionV1::BevClearInvariant => {
                    if !BEV_EXCEPTION_VECTOR_DESTINATIONS_V1.contains(&vector.destination) {
                        return Err(SourceFrontierError::InvalidBevClearVectorUnreachability {
                            destination: vector.destination,
                        });
                    }
                }
                ExceptionVectorDispositionV1::Open { reason } => {
                    if reason.trim().is_empty() {
                        return Err(SourceFrontierError::EmptyExceptionVectorOpenReason {
                            destination: vector.destination,
                        });
                    }
                }
            }
        }
        for destination in MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1 {
            if !input
                .exception_vectors
                .iter()
                .any(|vector| vector.destination == destination)
            {
                return Err(SourceFrontierError::MissingModeledExceptionVector { destination });
            }
        }
        sort_dedup(&mut input.cpu_store_watched_destinations);
        if input
            .cpu_store_watched_destinations
            .iter()
            .any(|destination| !destination.is_multiple_of(4))
        {
            return Err(SourceFrontierError::InvalidConditionalCpuWordStore {
                bank: "<watched-destinations>".to_string(),
                site_pc: 0,
            });
        }
        for store in &mut input.conditional_cpu_word_stores {
            sort_dedup(&mut store.open_requirements);
            let writer = input.dense_generations.iter().find(|generation| {
                generation.name == store.writer_bank && generation.bank_id == store.writer_bank_id
            });
            let source = input.dense_generations.iter().find(|generation| {
                generation.name == store.source_bank && generation.bank_id == store.source_bank_id
            });
            let writer_contains_site = writer.is_some_and(|generation| {
                generation.load_start <= store.site_pc
                    && store
                        .site_pc
                        .checked_add(4)
                        .is_some_and(|end| end <= generation.load_end)
            });
            let source_contains_word = source.is_some_and(|generation| {
                generation.load_start <= store.source_address
                    && store
                        .source_address
                        .checked_add(4)
                        .is_some_and(|end| end <= generation.load_end)
            });
            let required_conditions = [
                ConditionalCpuWordStoreRequirementV1::SourceStableUntilLoad,
                ConditionalCpuWordStoreRequirementV1::StoreSiteExecutes,
            ];
            if !writer_contains_site
                || !source_contains_word
                || store.source_value != store.value
                || !store.destination.is_multiple_of(4)
                || !input
                    .cpu_store_watched_destinations
                    .contains(&store.destination)
                || store.open_requirements.as_slice() != required_conditions
            {
                return Err(SourceFrontierError::InvalidConditionalCpuWordStore {
                    bank: store.writer_bank.clone(),
                    site_pc: store.site_pc,
                });
            }
        }
        for store in &mut input.open_cpu_word_stores {
            sort_dedup(&mut store.blockers);
            let writer_contains_site = input.dense_generations.iter().any(|generation| {
                generation.name == store.writer_bank
                    && generation.bank_id == store.writer_bank_id
                    && generation.load_start <= store.site_pc
                    && store
                        .site_pc
                        .checked_add(4)
                        .is_some_and(|end| end <= generation.load_end)
            });
            if !writer_contains_site || store.blockers.is_empty() {
                return Err(SourceFrontierError::InvalidOpenCpuWordStore {
                    bank: store.writer_bank.clone(),
                    site_pc: store.site_pc,
                });
            }
        }
        sort_dedup(&mut input.cpu_store_scans);
        for scan in &input.cpu_store_scans {
            let generation_exists = input.dense_generations.iter().any(|generation| {
                generation.name == scan.bank && generation.bank_id == scan.bank_id
            });
            let conditional_count = input
                .conditional_cpu_word_stores
                .iter()
                .filter(|store| {
                    store.writer_bank == scan.bank && store.writer_bank_id == scan.bank_id
                })
                .count();
            let open_count = input
                .open_cpu_word_stores
                .iter()
                .filter(|store| {
                    store.writer_bank == scan.bank && store.writer_bank_id == scan.bank_id
                })
                .count();
            if !generation_exists
                || conditional_count != scan.conditional_store_count as usize
                || open_count != scan.open_store_count as usize
            {
                return Err(SourceFrontierError::InvalidCpuStoreScan {
                    bank: scan.bank.clone(),
                    bank_id: scan.bank_id,
                });
            }
        }
        if !input.cpu_store_watched_destinations.is_empty() {
            for generation in &input.dense_generations {
                let matching_scans = input
                    .cpu_store_scans
                    .iter()
                    .filter(|scan| {
                        scan.bank == generation.name && scan.bank_id == generation.bank_id
                    })
                    .count();
                if matching_scans != 1 {
                    return Err(SourceFrontierError::MissingCpuStoreScan {
                        bank: generation.name.clone(),
                        bank_id: generation.bank_id,
                    });
                }
            }
        }
        for scan in &mut input.cop0_status_scans {
            sort_dedup(&mut scan.proven_code_writes);
            sort_dedup(&mut scan.proven_data_words);
            sort_dedup(&mut scan.unclassified_writes);
            for proof in &mut scan.proven_code_value_proofs {
                normalize_cop0_status_value_proof(proof);
            }
            sort_dedup(&mut scan.proven_code_value_proofs);
            sort_dedup(&mut scan.open_indirect_sites);
            let Some(generation) = input.dense_generations.iter().find(|generation| {
                generation.name == scan.bank && generation.bank_id == scan.bank_id
            }) else {
                return Err(SourceFrontierError::InvalidCop0StatusScan {
                    bank: scan.bank.clone(),
                    bank_id: scan.bank_id,
                });
            };
            let expected_words = (generation.load_end - generation.load_start) / 4;
            let mut seen_sites = BTreeSet::new();
            let sites_are_valid = scan
                .proven_code_writes
                .iter()
                .chain(&scan.proven_data_words)
                .chain(&scan.unclassified_writes)
                .all(|site| {
                    (generation.load_start..generation.load_end).contains(&site.site_pc)
                        && site.site_pc.is_multiple_of(4)
                        && valid_cop0_status_site(site)
                        && seen_sites.insert(site.site_pc)
                });
            let indirect_sites_are_valid = scan.open_indirect_sites.iter().all(|site_pc| {
                (generation.load_start..generation.load_end).contains(site_pc)
                    && site_pc.is_multiple_of(4)
            });
            if scan.aligned_word_count != expected_words
                || !sites_are_valid
                || !valid_cop0_status_value_proofs(
                    &scan.proven_code_writes,
                    &scan.proven_code_value_proofs,
                )
                || !indirect_sites_are_valid
            {
                return Err(SourceFrontierError::InvalidCop0StatusScan {
                    bank: scan.bank.clone(),
                    bank_id: scan.bank_id,
                });
            }
        }
        for generation in &input.dense_generations {
            let matching_scans = input
                .cop0_status_scans
                .iter()
                .filter(|scan| scan.bank == generation.name && scan.bank_id == generation.bank_id)
                .count();
            if matching_scans != 1 {
                return Err(SourceFrontierError::MissingCop0StatusScan {
                    bank: generation.name.clone(),
                    bank_id: generation.bank_id,
                });
            }
        }
        if input.exception_vectors.iter().any(|vector| {
            matches!(
                &vector.disposition,
                ExceptionVectorDispositionV1::Open { .. }
            )
        }) {
            input
                .open_writer_classes
                .push(OpenWriterClass::UnadmittedExceptionOrBevVector);
        }
        if !input.conditional_cpu_word_stores.is_empty() || !input.open_cpu_word_stores.is_empty() {
            input
                .open_writer_classes
                .push(OpenWriterClass::CpuCopyStoreOrDecompression);
        }
        for finding in &input.direct_dma_findings {
            if finding.device_start >= finding.device_end
                || finding.rdram_start >= finding.rdram_end
                || finding.device_end - finding.device_start
                    != finding.rdram_end - finding.rdram_start
            {
                return Err(SourceFrontierError::InvalidDirectDmaRange {
                    caller_pc: finding.caller_pc,
                });
            }
        }

        for blocker in &input.direct_dma_blockers {
            input.open_writer_classes.push(blocker.writer_class);
        }
        for primitive in &mut input.raw_pi_primitives {
            sort_dedup(&mut primitive.register_site_pcs);
            sort_dedup(&mut primitive.callers);
            for caller in &primitive.callers {
                match caller.resolution {
                    WriterResolutionV1::Bounded | WriterResolutionV1::Open => input
                        .open_writer_classes
                        .push(OpenWriterClass::IndirectPiEpiCall),
                    WriterResolutionV1::AdmittedDenseGeneration
                    | WriterResolutionV1::AdmittedExternalImage
                    | WriterResolutionV1::ProvenNonExecutable => {}
                }
            }
        }

        sort_dedup(&mut input.external_images);
        sort_dedup(&mut input.dense_generations);
        sort_dedup(&mut input.host_bindings);
        sort_dedup(&mut input.cache_sites);
        sort_dedup(&mut input.direct_dma_findings);
        sort_dedup(&mut input.direct_dma_blockers);
        sort_dedup(&mut input.raw_pi_primitives);
        sort_dedup(&mut input.cpu_store_scans);
        sort_dedup(&mut input.cop0_status_scans);
        sort_dedup(&mut input.conditional_cpu_word_stores);
        sort_dedup(&mut input.open_cpu_word_stores);
        let transfer_scan = input.transfer_scan.into_evidence();
        sort_dedup(&mut input.open_writer_classes);

        let receipt = Self {
            schema: EXECUTABLE_SOURCE_FRONTIER_SCHEMA_V1.to_string(),
            producer: input.producer,
            normalized_rom_sha256: input.normalized_rom_sha256,
            dense_aot_pack_sha256: input.dense_aot_pack_sha256,
            initial_cop0_status: input.initial_cop0_status,
            dense_generations: input.dense_generations,
            external_images: input.external_images,
            exception_vectors: input.exception_vectors,
            host_bindings: input.host_bindings,
            cache_sites: input.cache_sites,
            direct_dma_findings: input.direct_dma_findings,
            direct_dma_blockers: input.direct_dma_blockers,
            raw_pi_primitives: input.raw_pi_primitives,
            cpu_store_watched_destinations: input.cpu_store_watched_destinations,
            cpu_store_scans: input.cpu_store_scans,
            cop0_status_scans: input.cop0_status_scans,
            external_cop0_status_scans: input.external_cop0_status_scans,
            conditional_cpu_word_stores: input.conditional_cpu_word_stores,
            open_cpu_word_stores: input.open_cpu_word_stores,
            transfer_scan,
            open_writer_classes: input.open_writer_classes,
        };
        if let Some(vector) = receipt.exception_vectors.iter().find(|vector| {
            matches!(
                &vector.disposition,
                ExceptionVectorDispositionV1::BevClearInvariant
            ) && !receipt.validates_bev_clear_invariant()
        }) {
            return Err(SourceFrontierError::InvalidBevClearVectorUnreachability {
                destination: vector.destination,
            });
        }
        Ok(receipt)
    }

    /// Serialize the normalized receipt without presentation whitespace.
    /// Struct field order is the wire order and every collection was sorted
    /// and deduplicated by [`Self::new`].
    pub fn canonical_json_bytes(&self) -> Result<Vec<u8>, SourceFrontierError> {
        serde_json::to_vec(self)
            .map_err(|error| SourceFrontierError::CanonicalJson(error.to_string()))
    }

    pub fn canonical_sha256(&self) -> Result<String, SourceFrontierError> {
        Ok(format!(
            "{:x}",
            Sha256::digest(self.canonical_json_bytes()?)
        ))
    }

    fn status_bev_sources_are_closed(&self) -> bool {
        self.initial_cop0_status.bev_is_proven_clear()
            && self.host_bindings.iter().all(|binding| {
                binding.current_status_effect
                    != HostCurrentStatusEffectV1::CBridgeCopyBackUnclassified
            })
            && self.cop0_status_scans.iter().all(|scan| {
                scan.unclassified_writes.is_empty()
                    && scan.open_indirect_sites.is_empty()
                    && scan
                        .proven_code_value_proofs
                        .iter()
                        .all(cop0_status_value_proof_is_bev_clear)
            })
            && self.external_cop0_status_scans.iter().all(|scan| {
                scan.unclassified_writes.is_empty()
                    && scan.open_indirect_sites.is_empty()
                    && scan
                        .proven_code_value_proofs
                        .iter()
                        .all(cop0_status_value_proof_is_bev_clear)
            })
    }

    fn validates_bev_clear_invariant(&self) -> bool {
        self.status_bev_sources_are_closed()
            && self.exception_vectors.iter().all(|vector| {
                BEV_EXCEPTION_VECTOR_DESTINATIONS_V1.contains(&vector.destination)
                    || matches!(
                        &vector.disposition,
                        ExceptionVectorDispositionV1::ExactCodeOwner(_)
                    )
            })
            && self.open_writer_classes.is_empty()
            && self.conditional_cpu_word_stores.is_empty()
            && self.open_cpu_word_stores.is_empty()
            && self.direct_dma_blockers.is_empty()
            && self
                .cache_sites
                .iter()
                .all(|site| !matches!(site.disposition, CacheSiteDispositionV1::Unclassified))
            && self.raw_pi_primitives.iter().all(|primitive| {
                primitive.callers.iter().all(|caller| {
                    !matches!(
                        caller.resolution,
                        WriterResolutionV1::Bounded | WriterResolutionV1::Open
                    )
                })
            })
            && self.transfer_scan.summary.indirect_bounded == 0
            && self.transfer_scan.summary.indirect_open == 0
            && self.transfer_scan.summary.direct_open == 0
            && self.transfer_scan.inventory == TransferInventoryV1::Complete
            && self
                .transfer_scan
                .indirect_frontier
                .iter()
                .all(|site| matches!(site.disposition, IndirectDispositionV1::Closed))
    }

    /// Whether this inventory still contains any explicit open frontier.
    ///
    /// The inverse is deliberately not named or exposed as exhaustiveness:
    /// an empty supplied inventory cannot prove that its producer enumerated
    /// every mechanism.
    pub fn has_open_frontier(&self) -> bool {
        !self.initial_cop0_status.bev_is_proven_clear()
            || self.exception_vectors.iter().any(|vector| {
                matches!(
                    &vector.disposition,
                    ExceptionVectorDispositionV1::Open { .. }
                )
            })
            || !self.open_writer_classes.is_empty()
            || self.host_bindings.iter().any(|binding| {
                binding.current_status_effect
                    == HostCurrentStatusEffectV1::CBridgeCopyBackUnclassified
            })
            || self.cop0_status_scans.iter().any(|scan| {
                !scan.unclassified_writes.is_empty()
                    || !scan.open_indirect_sites.is_empty()
                    || !scan
                        .proven_code_value_proofs
                        .iter()
                        .all(cop0_status_value_proof_is_bev_clear)
            })
            || self.external_cop0_status_scans.iter().any(|scan| {
                !scan.unclassified_writes.is_empty()
                    || !scan.open_indirect_sites.is_empty()
                    || !scan
                        .proven_code_value_proofs
                        .iter()
                        .all(cop0_status_value_proof_is_bev_clear)
            })
            || !self.conditional_cpu_word_stores.is_empty()
            || !self.open_cpu_word_stores.is_empty()
            || !self.direct_dma_blockers.is_empty()
            || self
                .cache_sites
                .iter()
                .any(|site| matches!(site.disposition, CacheSiteDispositionV1::Unclassified))
            || !self.raw_pi_primitives.iter().all(|primitive| {
                primitive.callers.iter().all(|caller| {
                    !matches!(
                        caller.resolution,
                        WriterResolutionV1::Bounded | WriterResolutionV1::Open
                    )
                })
            })
            || self.transfer_scan.summary.indirect_bounded != 0
            || self.transfer_scan.summary.indirect_open != 0
            || self.transfer_scan.summary.direct_open != 0
            || self.transfer_scan.inventory == TransferInventoryV1::Open
            || !self
                .transfer_scan
                .indirect_frontier
                .iter()
                .all(|site| matches!(site.disposition, IndirectDispositionV1::Closed))
    }
}

#[cfg(test)]
mod tests;
