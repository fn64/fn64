//! Project-owned certification denominators.
//!
//! A profile is code, not input: callers can select this fixed definition but
//! cannot deserialize a smaller requirement set and retain the same identity.

use sha2::{Digest, Sha256};
use std::fmt;

pub const FULL_PARITY_V1_SCHEMA: &str = "fn64.certification-profile.full-parity.v1";
pub const FULL_PARITY_V1_DEFINITION_SHA256: &str =
    "883709fe804cc05363fbcb66b8a93b1b684693fcd4bcb8835c6a202f2d60dfc0";

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CertificationRequirementClass {
    RomClass,
    TvRegion,
    ProgramRendererLane,
    Save,
    Controller,
    PublicMicrocode,
    RspRdpMechanism,
    PlatformApiTarget,
    Rt64TargetCase,
    PlatformTargetBlocker,
}

/// Exact identity of the project-owned denominator selected by a matrix.
///
/// This is a reference, not a caller-defined policy. Deserialization is
/// permitted so manifests can name the profile; [`Self::verify`] accepts only
/// the schema and golden definition digest compiled into this crate.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationProfileIdentity {
    pub schema: String,
    pub definition_sha256: String,
}

impl CertificationProfileIdentity {
    pub fn full_parity_v1() -> Self {
        Self {
            schema: FULL_PARITY_V1_SCHEMA.to_owned(),
            definition_sha256: FULL_PARITY_V1_DEFINITION_SHA256.to_owned(),
        }
    }

    pub fn verify(&self) -> Result<FullParityV1, CertificationProfileError> {
        if self.schema != FULL_PARITY_V1_SCHEMA {
            return Err(CertificationProfileError::UnsupportedSchema(
                self.schema.clone(),
            ));
        }
        if self.definition_sha256 != FULL_PARITY_V1_DEFINITION_SHA256 {
            return Err(CertificationProfileError::DefinitionDigestMismatch {
                stored: self.definition_sha256.clone(),
                expected: FULL_PARITY_V1_DEFINITION_SHA256.to_owned(),
            });
        }
        Ok(FullParityV1::new())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationRequirementRef {
    class: CertificationRequirementClass,
    id: String,
}

impl CertificationRequirementRef {
    pub(crate) fn from_requirement(requirement: &CertificationRequirement) -> Self {
        Self {
            class: requirement.class(),
            id: requirement.id().to_owned(),
        }
    }

    pub const fn class(&self) -> CertificationRequirementClass {
        self.class
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn verify_member(
        &self,
        profile: FullParityV1,
    ) -> Result<(), CertificationProfileError> {
        if profile
            .requirements()
            .iter()
            .any(|requirement| requirement.class() == self.class && requirement.id() == self.id)
        {
            Ok(())
        } else {
            Err(CertificationProfileError::UnknownRequirement {
                class: self.class,
                id: self.id.clone(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificationProfileError {
    UnsupportedSchema(String),
    DefinitionDigestMismatch {
        stored: String,
        expected: String,
    },
    UnknownRequirement {
        class: CertificationRequirementClass,
        id: String,
    },
}

impl fmt::Display for CertificationProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported certification profile {schema:?}")
            }
            Self::DefinitionDigestMismatch { stored, expected } => write!(
                formatter,
                "certification profile definition SHA mismatch: stored={stored}, expected={expected}"
            ),
            Self::UnknownRequirement { class, id } => write!(
                formatter,
                "unknown certification requirement ({}, {id:?})",
                class.as_str()
            ),
        }
    }
}

impl std::error::Error for CertificationProfileError {}

impl CertificationRequirementClass {
    const ALL: [Self; 10] = [
        Self::RomClass,
        Self::TvRegion,
        Self::ProgramRendererLane,
        Self::Save,
        Self::Controller,
        Self::PublicMicrocode,
        Self::RspRdpMechanism,
        Self::PlatformApiTarget,
        Self::Rt64TargetCase,
        Self::PlatformTargetBlocker,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RomClass => "rom_class",
            Self::TvRegion => "tv_region",
            Self::ProgramRendererLane => "program_renderer_lane",
            Self::Save => "save",
            Self::Controller => "controller",
            Self::PublicMicrocode => "public_microcode",
            Self::RspRdpMechanism => "rsp_rdp_mechanism",
            Self::PlatformApiTarget => "platform_api_target",
            Self::Rt64TargetCase => "rt64_target_case",
            Self::PlatformTargetBlocker => "platform_target_blocker",
        }
    }
}

/// One immutable member of a project-owned certification denominator.
///
/// Fields and construction stay private so downstream code can inspect the
/// fixed profile but cannot manufacture a requirement with an arbitrary ID.
///
/// ```compile_fail
/// use fn64_boot_harness::{CertificationRequirement, CertificationRequirementClass};
/// let _ = CertificationRequirement {
///     class: CertificationRequirementClass::Save,
///     id: "only-my-game".to_owned(),
/// };
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CertificationRequirement {
    class: CertificationRequirementClass,
    id: String,
}

impl CertificationRequirement {
    fn new(class: CertificationRequirementClass, id: impl Into<String>) -> Self {
        Self {
            class,
            id: id.into(),
        }
    }

    pub const fn class(&self) -> CertificationRequirementClass {
        self.class
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The complete parity denominator established by this source revision.
///
/// This zero-sized selector has no serialized form and no caller-provided
/// fields. Its canonical digest covers the schema, category boundaries,
/// counts, order, and every immutable requirement ID below.
///
/// ```compile_fail
/// use fn64_boot_harness::FullParityV1;
/// let _ = FullParityV1 { _private: () };
/// ```
///
/// ```compile_fail
/// use fn64_boot_harness::FullParityV1;
/// let _: FullParityV1 = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FullParityV1 {
    _private: (),
}

impl FullParityV1 {
    pub const REQUIREMENT_COUNT: usize = 162;

    pub const fn new() -> Self {
        Self { _private: () }
    }

    pub const fn schema(self) -> &'static str {
        FULL_PARITY_V1_SCHEMA
    }

    pub const fn definition_sha256(self) -> &'static str {
        FULL_PARITY_V1_DEFINITION_SHA256
    }

    pub fn requirements(self) -> Vec<CertificationRequirement> {
        full_parity_v1_requirements()
    }

    pub fn recompute_definition_sha256(self) -> String {
        let mut wire = Vec::new();
        push_bytes(&mut wire, FULL_PARITY_V1_SCHEMA.as_bytes());
        for class in CertificationRequirementClass::ALL {
            push_bytes(&mut wire, class.as_str().as_bytes());
            let requirements = self.requirements();
            let count = requirements
                .iter()
                .filter(|requirement| requirement.class == class)
                .count();
            wire.extend_from_slice(
                &u32::try_from(count)
                    .expect("FullParityV1 category count exceeds u32")
                    .to_be_bytes(),
            );
            for requirement in requirements
                .iter()
                .filter(|requirement| requirement.class == class)
            {
                push_bytes(&mut wire, requirement.id.as_bytes());
            }
        }
        hex(&Sha256::digest(wire))
    }
}

impl Default for FullParityV1 {
    fn default() -> Self {
        Self::new()
    }
}

fn requirement(class: CertificationRequirementClass, id: &'static str) -> CertificationRequirement {
    CertificationRequirement::new(class, id)
}

fn full_parity_v1_requirements() -> Vec<CertificationRequirement> {
    let mut requirements = vec![
        // Synthetic fixtures remain useful mechanism evidence, but cannot satisfy
        // either executable class in the full-ROM denominator.
        requirement(CertificationRequirementClass::RomClass, "retail_cartridge"),
        requirement(CertificationRequirementClass::RomClass, "public_homebrew"),
        requirement(CertificationRequirementClass::TvRegion, "ntsc"),
        requirement(CertificationRequirementClass::TvRegion, "pal"),
        requirement(CertificationRequirementClass::TvRegion, "mpal"),
        requirement(
            CertificationRequirementClass::ProgramRendererLane,
            "native_archive/reference_lle_accuracy",
        ),
        requirement(
            CertificationRequirementClass::ProgramRendererLane,
            "native_archive/rt64_lle_accuracy",
        ),
        requirement(
            CertificationRequirementClass::ProgramRendererLane,
            "typed_observed_function/reference_lle_accuracy",
        ),
        requirement(
            CertificationRequirementClass::ProgramRendererLane,
            "typed_observed_function/rt64_lle_accuracy",
        ),
        requirement(
            CertificationRequirementClass::ProgramRendererLane,
            "typed_block/reference_lle_accuracy",
        ),
        requirement(
            CertificationRequirementClass::ProgramRendererLane,
            "typed_block/rt64_lle_accuracy",
        ),
        requirement(CertificationRequirementClass::Save, "no_cartridge_save"),
        requirement(CertificationRequirementClass::Save, "eeprom_4_kbit"),
        requirement(CertificationRequirementClass::Save, "eeprom_16_kbit"),
        requirement(CertificationRequirementClass::Save, "sram_32_kib"),
        requirement(CertificationRequirementClass::Save, "flash_ram_128_kib"),
        requirement(
            CertificationRequirementClass::Controller,
            "standard_controller",
        ),
        requirement(CertificationRequirementClass::Controller, "controller_pak"),
        requirement(CertificationRequirementClass::Controller, "rumble_pak"),
        requirement(CertificationRequirementClass::Controller, "transfer_pak"),
        requirement(
            CertificationRequirementClass::Controller,
            "voice_recognition_unit",
        ),
        requirement(CertificationRequirementClass::PublicMicrocode, "fast3d"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dex"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dlx"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dlx-rej"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dex2"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dex2-non"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dex2-rej"),
        requirement(CertificationRequirementClass::PublicMicrocode, "f3dlx2-rej"),
        requirement(CertificationRequirementClass::PublicMicrocode, "s2dex"),
        requirement(CertificationRequirementClass::PublicMicrocode, "s2dex2"),
        requirement(CertificationRequirementClass::PublicMicrocode, "l3dex"),
        requirement(CertificationRequirementClass::PublicMicrocode, "l3dex2"),
        requirement(CertificationRequirementClass::RspRdpMechanism, "dram-dpc"),
        requirement(CertificationRequirementClass::RspRdpMechanism, "xbus-dpc"),
        requirement(
            CertificationRequirementClass::RspRdpMechanism,
            "imem-replacement",
        ),
    ];
    for target in PLATFORM_TARGET_IDS {
        requirements.push(CertificationRequirement::new(
            CertificationRequirementClass::PlatformApiTarget,
            target,
        ));
    }
    for target in PLATFORM_TARGET_IDS {
        for case in RT64_CASE_IDS {
            requirements.push(CertificationRequirement::new(
                CertificationRequirementClass::Rt64TargetCase,
                format!("{target}/{case}"),
            ));
        }
    }
    for target in PLATFORM_TARGET_IDS {
        for blocker in PLATFORM_BLOCKER_IDS {
            requirements.push(CertificationRequirement::new(
                CertificationRequirementClass::PlatformTargetBlocker,
                format!("{target}/{blocker}"),
            ));
        }
    }
    assert_eq!(
        requirements.len(),
        FullParityV1::REQUIREMENT_COUNT,
        "FullParityV1 source denominator and public count diverged"
    );
    requirements
}

const PLATFORM_TARGET_IDS: [&str; 6] = [
    "macos-metal",
    "linux-vulkan",
    "windows10-d3d12",
    "windows10-vulkan",
    "windows11-d3d12",
    "windows11-vulkan",
];

const RT64_CASE_IDS: [&str; 13] = [
    "backend-lifecycle",
    "resolution-downsample",
    "user-controls-rebuild",
    "enhancement-emulator-controls",
    "framebuffer-rdram-region",
    "framebuffer-enhancement",
    "texture-replacements",
    "latency-skip-buffering",
    "latency-present-early",
    "deferred-debugger",
    "ubershader-critical-path",
    "hfr-hle-cooperation",
    "extended-gbi-cooperation",
];

const PLATFORM_BLOCKER_IDS: [&str; 7] = [
    "recognized-hle-and-extended-gbi",
    "aspect-and-generated-frames",
    "remaining-user-controls",
    "remaining-enhancement-controls",
    "inspector-gui",
    "full-adapter-rom-coverage",
    "declared-host-range",
];

fn push_bytes(wire: &mut Vec<u8>, bytes: &[u8]) {
    wire.extend_from_slice(
        &u32::try_from(bytes.len())
            .expect("FullParityV1 definition field exceeds u32")
            .to_be_bytes(),
    );
    wire.extend_from_slice(bytes);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ids(class: CertificationRequirementClass) -> Vec<String> {
        FullParityV1::new()
            .requirements()
            .into_iter()
            .filter(|requirement| requirement.class() == class)
            .map(|requirement| requirement.id().to_owned())
            .collect()
    }

    #[test]
    fn full_parity_v1_has_the_fixed_nonshrinking_denominator() {
        let expected = [2, 3, 6, 5, 5, 12, 3, 6, 78, 42];
        let actual = CertificationRequirementClass::ALL.map(|class| ids(class).len());
        assert_eq!(actual, expected);
        assert_eq!(
            actual.into_iter().sum::<usize>(),
            FullParityV1::REQUIREMENT_COUNT
        );

        let unique: BTreeSet<_> = FullParityV1::new()
            .requirements()
            .into_iter()
            .map(|requirement| (requirement.class(), requirement.id().to_owned()))
            .collect();
        assert_eq!(unique.len(), FullParityV1::REQUIREMENT_COUNT);
    }

    #[test]
    fn release_matrix_catalog_ids_are_preserved() {
        let programs = [
            crate::ProgramFeature::NativeArchive,
            crate::ProgramFeature::TypedObservedFunction,
            crate::ProgramFeature::TypedBlock,
        ]
        .map(|value| serde_json::to_value(value).unwrap());
        let renderers = [
            crate::RendererFeature::ReferenceLleAccuracy,
            crate::RendererFeature::Rt64LleAccuracy,
        ]
        .map(|value| serde_json::to_value(value).unwrap());
        let lanes = ids(CertificationRequirementClass::ProgramRendererLane);
        for program in programs {
            for renderer in &renderers {
                let id = format!(
                    "{}/{}",
                    program.as_str().unwrap(),
                    renderer.as_str().unwrap()
                );
                assert!(lanes.contains(&id));
            }
        }

        let saves = [
            crate::SaveFeature::NoCartridgeSave,
            crate::SaveFeature::Eeprom4Kbit,
            crate::SaveFeature::Eeprom16Kbit,
            crate::SaveFeature::Sram32Kib,
            crate::SaveFeature::FlashRam128Kib,
        ]
        .map(|value| serde_json::to_value(value).unwrap());
        assert_eq!(
            saves
                .map(|value| value.as_str().unwrap().to_owned())
                .as_slice(),
            ids(CertificationRequirementClass::Save).as_slice()
        );

        let controllers = [
            crate::ControllerFeature::StandardController,
            crate::ControllerFeature::ControllerPak,
            crate::ControllerFeature::RumblePak,
            crate::ControllerFeature::TransferPak,
            crate::ControllerFeature::VoiceRecognitionUnit,
        ]
        .map(|value| serde_json::to_value(value).unwrap());
        assert_eq!(
            controllers
                .map(|value| value.as_str().unwrap().to_owned())
                .as_slice(),
            ids(CertificationRequirementClass::Controller).as_slice()
        );
    }

    #[test]
    fn platform_catalog_ids_are_preserved() {
        let catalog: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/rt64-platform-certification.json"
        ))
        .unwrap();
        let targets: Vec<_> = catalog["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned())
            .collect();
        let cases: Vec<_> = catalog["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned())
            .collect();
        let blockers: Vec<_> = catalog["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            targets,
            ids(CertificationRequirementClass::PlatformApiTarget)
        );

        let target_cases: Vec<_> = targets
            .iter()
            .flat_map(|target| cases.iter().map(move |case| format!("{target}/{case}")))
            .collect();
        assert_eq!(
            target_cases,
            ids(CertificationRequirementClass::Rt64TargetCase)
        );
        let target_blockers: Vec<_> = targets
            .iter()
            .flat_map(|target| {
                blockers
                    .iter()
                    .map(move |blocker| format!("{target}/{blocker}"))
            })
            .collect();
        assert_eq!(
            target_blockers,
            ids(CertificationRequirementClass::PlatformTargetBlocker)
        );
    }

    #[test]
    fn definition_digest_is_golden() {
        assert_eq!(
            FULL_PARITY_V1_DEFINITION_SHA256,
            "883709fe804cc05363fbcb66b8a93b1b684693fcd4bcb8835c6a202f2d60dfc0"
        );
        assert_eq!(
            FullParityV1::new().recompute_definition_sha256(),
            FULL_PARITY_V1_DEFINITION_SHA256
        );
    }

    #[test]
    fn serialized_profile_identity_cannot_select_a_smaller_definition() {
        CertificationProfileIdentity::full_parity_v1()
            .verify()
            .unwrap();

        let mut wrong_digest = CertificationProfileIdentity::full_parity_v1();
        wrong_digest.definition_sha256 = "00".repeat(32);
        assert!(matches!(
            wrong_digest.verify(),
            Err(CertificationProfileError::DefinitionDigestMismatch { .. })
        ));

        let wrong_schema = CertificationProfileIdentity {
            schema: "fn64.certification-profile.full-parity.v0".to_owned(),
            definition_sha256: FULL_PARITY_V1_DEFINITION_SHA256.to_owned(),
        };
        assert!(matches!(
            wrong_schema.verify(),
            Err(CertificationProfileError::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn requirement_membership_is_class_qualified() {
        let profile = FullParityV1::new();
        let correct = CertificationRequirementRef {
            class: CertificationRequirementClass::Save,
            id: "sram_32_kib".to_owned(),
        };
        correct.verify_member(profile).unwrap();

        let cross_labeled = CertificationRequirementRef {
            class: CertificationRequirementClass::Controller,
            id: "sram_32_kib".to_owned(),
        };
        assert!(matches!(
            cross_labeled.verify_member(profile),
            Err(CertificationProfileError::UnknownRequirement {
                class: CertificationRequirementClass::Controller,
                ..
            })
        ));
    }
}
