//! Backend-neutral, content-addressed graphics microcode admission.
//!
//! A task header or colliding opcode byte never selects a decoder family.
//! Callers must admit one exact complete IMEM image, and release evidence may
//! additionally require the exact text/data pair consumed by the task.
//!
//! Provenance: family names, command-envelope distinctions, vertex-cache
//! capacities, and loadable-family boundaries come from Nintendo's public
//! `gbi.h`/`gs2dex.h`, the public Fast3D, F3DEX, L3DEX, and S2DEX manuals, and
//! the project clean-room synthesis in `docs/MICROCODE-DENOMINATOR.md`.
//! F3DZEX2 remains named but unadmitted because those allowed sources do not
//! specify its family-specific continuation and branch commands.

use crate::{F3dzex2Variant, MicrocodeDataImageIdentity, RenderError, UcodeId};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Exact identity of one complete 4 KiB microcode text image.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct UcodeDigest([u8; 32]);

impl UcodeDigest {
    pub fn from_text(text: &[u8]) -> Self {
        Self(Sha256::digest(text).into())
    }

    pub const fn from_sha256(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl std::fmt::Display for UcodeDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// How one microcode generation became active during a graphics task.
///
/// The distinction is part of the ordered admission contract: a task has
/// exactly one entry generation followed by zero or more self-loads. Native
/// adapters must not collapse repeated addresses because `A -> B -> A` and a
/// same-address content replacement are behaviorally distinct generations.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TaskAdmissionSource {
    TaskEntry,
    SelfLoad,
}

/// Behavior-bearing microcode identity at one activation boundary.
///
/// `UcodeId` remains the public family denominator, while this type prevents
/// F3DZEX2 variants with different point-lighting behavior from collapsing to
/// the same executable admission identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TaskAdmissionUcode {
    Fast3d,
    F3dex,
    F3dlx,
    F3dlxRej,
    F3dex2,
    F3dex2NoN,
    F3dex2Rej,
    F3dlx2Rej,
    F3dzex2(F3dzex2Variant),
    S2dex,
    S2dex2,
    L3dex,
    L3dex2,
    Other(u32),
}

impl TaskAdmissionUcode {
    pub const fn from_family(family: UcodeId) -> Self {
        match family {
            UcodeId::Fast3d => Self::Fast3d,
            UcodeId::F3dex => Self::F3dex,
            UcodeId::F3dlx => Self::F3dlx,
            UcodeId::F3dlxRej => Self::F3dlxRej,
            UcodeId::F3dex2 => Self::F3dex2,
            UcodeId::F3dex2NoN => Self::F3dex2NoN,
            UcodeId::F3dex2Rej => Self::F3dex2Rej,
            UcodeId::F3dlx2Rej => Self::F3dlx2Rej,
            UcodeId::F3dzex2 => {
                panic!("F3DZEX2 task admission requires an exact typed variant")
            }
            UcodeId::S2dex => Self::S2dex,
            UcodeId::S2dex2 => Self::S2dex2,
            UcodeId::L3dex => Self::L3dex,
            UcodeId::L3dex2 => Self::L3dex2,
            UcodeId::Other(value) => Self::Other(value),
        }
    }

    pub const fn family(self) -> UcodeId {
        match self {
            Self::Fast3d => UcodeId::Fast3d,
            Self::F3dex => UcodeId::F3dex,
            Self::F3dlx => UcodeId::F3dlx,
            Self::F3dlxRej => UcodeId::F3dlxRej,
            Self::F3dex2 => UcodeId::F3dex2,
            Self::F3dex2NoN => UcodeId::F3dex2NoN,
            Self::F3dex2Rej => UcodeId::F3dex2Rej,
            Self::F3dlx2Rej => UcodeId::F3dlx2Rej,
            Self::F3dzex2(_) => UcodeId::F3dzex2,
            Self::S2dex => UcodeId::S2dex,
            Self::S2dex2 => UcodeId::S2dex2,
            Self::L3dex => UcodeId::L3dex,
            Self::L3dex2 => UcodeId::L3dex2,
            Self::Other(value) => UcodeId::Other(value),
        }
    }

    pub const fn f3dzex2_variant(self) -> Option<F3dzex2Variant> {
        match self {
            Self::F3dzex2(variant) => Some(variant),
            _ => None,
        }
    }

    const fn canonical_tags(self) -> (u8, u32) {
        match self {
            Self::Fast3d => (1, 0),
            Self::F3dex => (2, 0),
            Self::F3dlx => (3, 0),
            Self::F3dlxRej => (4, 0),
            Self::F3dex2 => (5, 0),
            Self::F3dex2NoN => (6, 0),
            Self::F3dex2Rej => (7, 0),
            Self::F3dlx2Rej => (8, 0),
            Self::F3dzex2(variant) => (9, variant.canonical_tag()),
            Self::S2dex => (10, 0),
            Self::S2dex2 => (11, 0),
            Self::L3dex => (12, 0),
            Self::L3dex2 => (13, 0),
            Self::Other(value) => (u8::MAX, value),
        }
    }
}

/// Exact identity expected at one ordered microcode activation boundary.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TaskAdmissionGeneration {
    pub source: TaskAdmissionSource,
    pub text_address: u32,
    pub data_address: u32,
    pub text_sha256: UcodeDigest,
    pub data: MicrocodeDataImageIdentity,
    pub ucode: TaskAdmissionUcode,
}

impl TaskAdmissionGeneration {
    pub const fn family(self) -> UcodeId {
        self.ucode.family()
    }

    fn validate(self, ordinal: usize) {
        assert!(
            self.text_address < 0x0100_0000 && self.text_address.is_multiple_of(8),
            "microcode admission generation {ordinal} text address {:#010x} must be an aligned physical RDRAM offset",
            self.text_address
        );
        assert!(
            self.data_address < 0x0100_0000 && self.data_address.is_multiple_of(8),
            "microcode admission generation {ordinal} data address {:#010x} must be an aligned physical RDRAM offset",
            self.data_address
        );
        assert!(
            self.data.bytes > 0
                && self.data.bytes <= fn64_runtime::RSP_MEMORY_BANK_SIZE as u32
                && self.data.bytes.is_multiple_of(8),
            "microcode admission generation {ordinal} data length {} must be a nonzero 64-bit multiple no larger than one 4 KiB DMEM bank",
            self.data.bytes
        );
    }
}

/// Immutable pre-commit admission plan for one native HLE task.
///
/// Construction validates ordering and physical DMA shape. The vector keeps
/// every generation, including exact duplicates, so a native observer can
/// consume it positionally and fail loudly on a missing, extra, reordered, or
/// content-divergent activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskAdmissionPlan {
    generations: Box<[TaskAdmissionGeneration]>,
}

impl TaskAdmissionPlan {
    pub fn new(
        entry: TaskAdmissionGeneration,
        self_loads: impl IntoIterator<Item = TaskAdmissionGeneration>,
    ) -> Self {
        assert_eq!(
            entry.source,
            TaskAdmissionSource::TaskEntry,
            "microcode admission plan generation zero must be the task entry"
        );
        let mut generations = vec![entry];
        for generation in self_loads {
            assert_eq!(
                generation.source,
                TaskAdmissionSource::SelfLoad,
                "microcode admission plan generations after zero must be self-loads"
            );
            generations.push(generation);
        }
        for (ordinal, generation) in generations.iter().copied().enumerate() {
            generation.validate(ordinal);
        }
        Self {
            generations: generations.into_boxed_slice(),
        }
    }

    pub fn generations(&self) -> &[TaskAdmissionGeneration] {
        &self.generations
    }

    pub fn entry(&self) -> TaskAdmissionGeneration {
        self.generations[0]
    }

    pub fn self_loads(&self) -> &[TaskAdmissionGeneration] {
        &self.generations[1..]
    }

    pub fn len(&self) -> usize {
        self.generations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.generations.is_empty()
    }

    /// Stable identity of the complete ordered logical admission contract.
    ///
    /// Native adapters may bind additional storage-layout windows to this
    /// identity, but cannot replace it with an address set or final state: the
    /// generation source and order are part of the hash domain.
    pub fn sha256(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"fn64-task-admission-plan-v2\0");
        hash.update(
            u64::try_from(self.generations.len())
                .expect("task admission generation count fits u64")
                .to_be_bytes(),
        );
        for generation in &self.generations {
            hash.update([match generation.source {
                TaskAdmissionSource::TaskEntry => 1,
                TaskAdmissionSource::SelfLoad => 2,
            }]);
            hash.update(generation.text_address.to_be_bytes());
            hash.update(generation.data_address.to_be_bytes());
            hash.update(generation.text_sha256.as_bytes());
            hash.update(generation.data.bytes.to_be_bytes());
            hash.update(generation.data.sha256);
            let (family, detail) = generation.ucode.canonical_tags();
            hash.update([family]);
            hash.update(detail.to_be_bytes());
        }
        hash.finalize().into()
    }
}

/// Public command-envelope identity for polygon and line geometry HLE.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GeometryWireFamily {
    Fast3d,
    F3dex,
    F3dlx,
    F3dlxRej,
    F3dex2,
    F3dex2NoN,
    F3dex2Rej,
    F3dlx2Rej,
    F3dzex2,
    L3dex,
    L3dex2,
}

impl GeometryWireFamily {
    pub const fn ucode_id(self) -> UcodeId {
        match self {
            Self::Fast3d => UcodeId::Fast3d,
            Self::F3dex => UcodeId::F3dex,
            Self::F3dlx => UcodeId::F3dlx,
            Self::F3dlxRej => UcodeId::F3dlxRej,
            Self::F3dex2 => UcodeId::F3dex2,
            Self::F3dex2NoN => UcodeId::F3dex2NoN,
            Self::F3dex2Rej => UcodeId::F3dex2Rej,
            Self::F3dlx2Rej => UcodeId::F3dlx2Rej,
            Self::F3dzex2 => UcodeId::F3dzex2,
            Self::L3dex => UcodeId::L3dex,
            Self::L3dex2 => UcodeId::L3dex2,
        }
    }

    #[doc(hidden)]
    pub const fn is_line(self) -> bool {
        matches!(self, Self::L3dex | Self::L3dex2)
    }

    #[doc(hidden)]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fast3d => "Fast3D",
            Self::F3dex => "F3DEX",
            Self::F3dlx => "F3DLX",
            Self::F3dlxRej => "F3DLX.Rej",
            Self::F3dex2 => "F3DEX2",
            Self::F3dex2NoN => "F3DEX2.NoN",
            Self::F3dex2Rej => "F3DEX2.Rej",
            Self::F3dlx2Rej => "F3DLX2.Rej",
            Self::F3dzex2 => "F3DZEX2",
            Self::L3dex => "L3DEX",
            Self::L3dex2 => "L3DEX2",
        }
    }

    #[doc(hidden)]
    pub const fn cache_capacity(self) -> usize {
        match self {
            Self::Fast3d => 16,
            // Pinned RT64's F3DZEX2 BranchW exposes a seven-bit cache index.
            // The inherited G_VTX end field is also seven bits, so ordinary
            // loads can populate through slot 126 while slot 127 remains a
            // representable malformed/unloaded BranchW selection.
            Self::F3dzex2 => 128,
            Self::F3dlxRej | Self::F3dex2Rej | Self::F3dlx2Rej => 64,
            Self::F3dex
            | Self::F3dlx
            | Self::F3dex2
            | Self::F3dex2NoN
            | Self::L3dex
            | Self::L3dex2 => 32,
        }
    }

    #[doc(hidden)]
    pub const fn uses_legacy_polygon_wire(self) -> bool {
        matches!(self, Self::F3dex | Self::F3dlx | Self::F3dlxRej)
    }

    #[doc(hidden)]
    pub const fn is_legacy_loadable(self) -> bool {
        matches!(
            self,
            Self::F3dex | Self::F3dlx | Self::F3dlxRej | Self::L3dex
        )
    }

    #[doc(hidden)]
    pub const fn max_vertex_load_count(self) -> usize {
        match self {
            Self::F3dzex2 => 127,
            Self::F3dex2Rej | Self::F3dlx2Rej => 64,
            _ => 32,
        }
    }

    #[doc(hidden)]
    pub const fn is_reject(self) -> bool {
        matches!(self, Self::F3dlxRej | Self::F3dex2Rej | Self::F3dlx2Rej)
    }

    #[doc(hidden)]
    pub const fn has_unpublished_wire(self) -> bool {
        matches!(self, Self::F3dzex2)
    }
}

/// Behavior-bearing identity for one active geometry microcode generation.
///
/// [`GeometryWireFamily`] identifies the command envelope, while this profile
/// retains the exact [`TaskAdmissionUcode`] needed by behavior variants that
/// share that envelope. Construction does not admit a microcode image: the
/// catalog remains the sole production HLE-admission authority.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeometryUcodeProfile(TaskAdmissionUcode);

impl GeometryUcodeProfile {
    /// Construct the profile for one catalog-admitted public wire family.
    ///
    /// F3DZEX2 cannot pass through this broad-family constructor because its
    /// executable identity requires an exact classified variant.
    pub const fn from_public_family(family: GeometryWireFamily) -> Self {
        match family {
            GeometryWireFamily::F3dzex2 => {
                panic!("F3DZEX2 geometry profile requires an exact typed variant")
            }
            _ => Self(TaskAdmissionUcode::from_family(family.ucode_id())),
        }
    }

    /// Convert an already-typed admission identity into a geometry profile.
    ///
    /// This is an identity conversion, not an admission decision. Non-geometry
    /// microcodes have no geometry command profile and return `None`.
    pub const fn from_admission_ucode(ucode: TaskAdmissionUcode) -> Option<Self> {
        match ucode {
            TaskAdmissionUcode::Fast3d
            | TaskAdmissionUcode::F3dex
            | TaskAdmissionUcode::F3dlx
            | TaskAdmissionUcode::F3dlxRej
            | TaskAdmissionUcode::F3dex2
            | TaskAdmissionUcode::F3dex2NoN
            | TaskAdmissionUcode::F3dex2Rej
            | TaskAdmissionUcode::F3dlx2Rej
            | TaskAdmissionUcode::F3dzex2(_)
            | TaskAdmissionUcode::L3dex
            | TaskAdmissionUcode::L3dex2 => Some(Self(ucode)),
            TaskAdmissionUcode::S2dex
            | TaskAdmissionUcode::S2dex2
            | TaskAdmissionUcode::Other(_) => None,
        }
    }

    pub const fn wire_family(self) -> GeometryWireFamily {
        match self.0 {
            TaskAdmissionUcode::Fast3d => GeometryWireFamily::Fast3d,
            TaskAdmissionUcode::F3dex => GeometryWireFamily::F3dex,
            TaskAdmissionUcode::F3dlx => GeometryWireFamily::F3dlx,
            TaskAdmissionUcode::F3dlxRej => GeometryWireFamily::F3dlxRej,
            TaskAdmissionUcode::F3dex2 => GeometryWireFamily::F3dex2,
            TaskAdmissionUcode::F3dex2NoN => GeometryWireFamily::F3dex2NoN,
            TaskAdmissionUcode::F3dex2Rej => GeometryWireFamily::F3dex2Rej,
            TaskAdmissionUcode::F3dlx2Rej => GeometryWireFamily::F3dlx2Rej,
            TaskAdmissionUcode::F3dzex2(_) => GeometryWireFamily::F3dzex2,
            TaskAdmissionUcode::L3dex => GeometryWireFamily::L3dex,
            TaskAdmissionUcode::L3dex2 => GeometryWireFamily::L3dex2,
            TaskAdmissionUcode::S2dex
            | TaskAdmissionUcode::S2dex2
            | TaskAdmissionUcode::Other(_) => {
                panic!("non-geometry admission identity reached a geometry profile")
            }
        }
    }

    pub const fn admission_ucode(self) -> TaskAdmissionUcode {
        self.0
    }

    pub const fn family(self) -> UcodeId {
        self.0.family()
    }

    pub const fn f3dzex2_variant(self) -> Option<F3dzex2Variant> {
        self.0.f3dzex2_variant()
    }

    /// Whether the active microcode carries RT64's exact NoN capability.
    pub const fn no_n(self) -> bool {
        match self.0 {
            TaskAdmissionUcode::F3dex2NoN => true,
            TaskAdmissionUcode::F3dzex2(variant) => variant.no_near_clip(),
            _ => false,
        }
    }

    /// Whether the active variant can interpret point-light records.
    pub const fn point_lighting(self) -> bool {
        match self.0 {
            TaskAdmissionUcode::F3dzex2(variant) => variant.point_lighting(),
            _ => false,
        }
    }
}

/// Exact public polygon/line-family text images admitted for geometry HLE.
#[derive(Clone, Debug, Default)]
pub struct GeometryUcodeCatalog {
    digests: HashMap<UcodeDigest, GeometryWireFamily>,
    supported: Vec<UcodeId>,
}

impl GeometryUcodeCatalog {
    pub fn admit_sha256(&mut self, digest: [u8; 32]) {
        self.admit_sha256_for(GeometryWireFamily::F3dex2, digest);
    }

    pub fn admit_sha256_for(&mut self, family: GeometryWireFamily, digest: [u8; 32]) {
        self.admit(UcodeDigest::from_sha256(digest), family);
    }

    pub fn admit_text(&mut self, text: &[u8]) -> UcodeDigest {
        self.admit_text_for(GeometryWireFamily::F3dex2, text)
    }

    pub fn admit_text_for(&mut self, family: GeometryWireFamily, text: &[u8]) -> UcodeDigest {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "geometry microcode admission requires one complete 4 KiB IMEM image"
        );
        let digest = UcodeDigest::from_text(text);
        self.admit(digest, family);
        digest
    }

    fn admit(&mut self, digest: UcodeDigest, family: GeometryWireFamily) {
        assert!(
            !family.has_unpublished_wire(),
            "F3DZEX2 HLE admission requires a complete allowed-source specification for its family-specific continuation and branch commands"
        );
        if let Some(previous) = self.digests.get(&digest) {
            assert_eq!(
                *previous, family,
                "one geometry microcode digest cannot identify two wire families"
            );
            return;
        }
        self.digests.insert(digest, family);
        let ucode = family.ucode_id();
        if !self.supported.contains(&ucode) {
            self.supported.push(ucode);
            self.supported.sort_by_key(ucode_sort_key);
        }
    }

    pub fn require_text(&self, text: &[u8]) -> Result<GeometryWireFamily, RenderError> {
        let digest = UcodeDigest::from_text(text);
        self.family(digest).ok_or(RenderError::RequiresLle {
            ucode_sha256: digest.as_bytes(),
        })
    }

    pub fn require_profile_text(&self, text: &[u8]) -> Result<GeometryUcodeProfile, RenderError> {
        self.require_text(text)
            .map(GeometryUcodeProfile::from_public_family)
    }

    #[doc(hidden)]
    pub fn family(&self, digest: UcodeDigest) -> Option<GeometryWireFamily> {
        self.digests.get(&digest).copied()
    }

    #[doc(hidden)]
    pub fn profile(&self, digest: UcodeDigest) -> Option<GeometryUcodeProfile> {
        self.family(digest)
            .map(GeometryUcodeProfile::from_public_family)
    }

    pub fn identify_text(
        &self,
        text: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        self.family(UcodeDigest::from_text(text))
            .map(GeometryWireFamily::ucode_id)
    }

    pub fn supported_ucodes(&self) -> &[UcodeId] {
        &self.supported
    }
}

/// Source-compatible name for F3DEX2-only callers.
pub type F3dex2UcodeCatalog = GeometryUcodeCatalog;

/// Public `gs2dex.h` wire family selected by exact text identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum S2dexWireFamily {
    S2dex,
    S2dex2,
}

impl S2dexWireFamily {
    pub const fn ucode_id(self) -> UcodeId {
        match self {
            Self::S2dex => UcodeId::S2dex,
            Self::S2dex2 => UcodeId::S2dex2,
        }
    }
}

const S2DEX_SUPPORTED: &[UcodeId] = &[UcodeId::S2dex, UcodeId::S2dex2];
const S2DEX_ONLY: &[UcodeId] = &[UcodeId::S2dex];
const S2DEX2_ONLY: &[UcodeId] = &[UcodeId::S2dex2];

#[derive(Clone, Debug, Default)]
pub struct S2dexUcodeCatalog {
    digests: HashMap<UcodeDigest, S2dexWireFamily>,
}

impl S2dexUcodeCatalog {
    pub fn admit_sha256(&mut self, digest: [u8; 32]) {
        self.admit_sha256_for(S2dexWireFamily::S2dex2, digest);
    }

    pub fn admit_sha256_for(&mut self, family: S2dexWireFamily, digest: [u8; 32]) {
        self.admit(UcodeDigest::from_sha256(digest), family);
    }

    pub fn admit_text(&mut self, text: &[u8]) -> UcodeDigest {
        self.admit_text_for(S2dexWireFamily::S2dex2, text)
    }

    pub fn admit_text_for(&mut self, family: S2dexWireFamily, text: &[u8]) -> UcodeDigest {
        assert_eq!(
            text.len(),
            fn64_runtime::RSP_MEMORY_BANK_SIZE,
            "S2DEX microcode admission requires one complete 4 KiB IMEM image"
        );
        let digest = UcodeDigest::from_text(text);
        self.admit(digest, family);
        digest
    }

    fn admit(&mut self, digest: UcodeDigest, family: S2dexWireFamily) {
        if let Some(previous) = self.digests.get(&digest) {
            assert_eq!(
                *previous, family,
                "one S2DEX microcode digest cannot identify two wire families"
            );
            return;
        }
        self.digests.insert(digest, family);
    }

    pub fn require_text(&self, text: &[u8]) -> Result<S2dexWireFamily, RenderError> {
        let digest = UcodeDigest::from_text(text);
        self.digests
            .get(&digest)
            .copied()
            .ok_or(RenderError::RequiresLle {
                ucode_sha256: digest.as_bytes(),
            })
    }

    pub fn identify_text(
        &self,
        text: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        self.digests
            .get(&UcodeDigest::from_text(text))
            .copied()
            .map(S2dexWireFamily::ucode_id)
    }

    pub fn supported_ucodes(&self) -> &'static [UcodeId] {
        let s2dex = self
            .digests
            .values()
            .any(|family| *family == S2dexWireFamily::S2dex);
        let s2dex2 = self
            .digests
            .values()
            .any(|family| *family == S2dexWireFamily::S2dex2);
        match (s2dex, s2dex2) {
            (false, false) => &[],
            (true, false) => S2DEX_ONLY,
            (false, true) => S2DEX2_ONLY,
            (true, true) => S2DEX_SUPPORTED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MicrocodePairKey {
    text_sha256: [u8; 32],
    data: MicrocodeDataImageIdentity,
}

/// Exact text/data pairs admitted to release microcode-family evidence.
#[derive(Clone, Debug, Default)]
pub struct MicrocodePairCatalog {
    families: HashMap<MicrocodePairKey, UcodeId>,
}

impl MicrocodePairCatalog {
    pub fn admit(
        &mut self,
        family: UcodeId,
        text_sha256: [u8; 32],
        data: MicrocodeDataImageIdentity,
    ) {
        let key = MicrocodePairKey { text_sha256, data };
        if let Some(previous) = self.families.get(&key) {
            assert_eq!(
                *previous, family,
                "one exact microcode text/data pair cannot identify two families"
            );
            return;
        }
        self.families.insert(key, family);
    }

    pub fn identify(
        &self,
        text: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        data: MicrocodeDataImageIdentity,
    ) -> Option<UcodeId> {
        self.families
            .get(&MicrocodePairKey {
                text_sha256: Sha256::digest(text).into(),
                data,
            })
            .copied()
    }
}

fn ucode_sort_key(ucode: &UcodeId) -> u8 {
    match ucode {
        UcodeId::Fast3d => 0,
        UcodeId::F3dex => 1,
        UcodeId::F3dlx => 2,
        UcodeId::F3dlxRej => 3,
        UcodeId::F3dex2 => 4,
        UcodeId::F3dex2NoN => 5,
        UcodeId::F3dex2Rej => 6,
        UcodeId::F3dlx2Rej => 7,
        UcodeId::F3dzex2 => 8,
        UcodeId::L3dex => 9,
        UcodeId::L3dex2 => 10,
        UcodeId::S2dex => 11,
        UcodeId::S2dex2 => 12,
        UcodeId::Other(_) => 13,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission_generation(
        source: TaskAdmissionSource,
        text_address: u32,
        digest_byte: u8,
    ) -> TaskAdmissionGeneration {
        TaskAdmissionGeneration {
            source,
            text_address,
            data_address: text_address + 0x1000,
            text_sha256: UcodeDigest::from_sha256([digest_byte; 32]),
            data: MicrocodeDataImageIdentity {
                bytes: 8,
                sha256: [digest_byte.wrapping_add(1); 32],
            },
            ucode: TaskAdmissionUcode::F3dex2,
        }
    }

    #[test]
    fn task_admission_plan_preserves_same_address_content_and_a_b_a_order() {
        let entry = admission_generation(TaskAdmissionSource::TaskEntry, 0x1000, 0x10);
        let a_changed = admission_generation(TaskAdmissionSource::SelfLoad, 0x1000, 0x11);
        let b = admission_generation(TaskAdmissionSource::SelfLoad, 0x2000, 0x20);
        let a_again = admission_generation(TaskAdmissionSource::SelfLoad, 0x1000, 0x10);
        let plan = TaskAdmissionPlan::new(entry, [a_changed, b, a_again]);

        assert_eq!(plan.len(), 4);
        assert!(!plan.is_empty());
        assert_eq!(plan.entry(), entry);
        assert_eq!(plan.self_loads(), &[a_changed, b, a_again]);
        assert_ne!(
            plan.generations()[0].text_sha256,
            plan.generations()[1].text_sha256
        );
        assert_eq!(
            plan.generations()[0].text_address,
            plan.generations()[1].text_address
        );
        assert_eq!(
            plan.generations()[0].text_sha256,
            plan.generations()[3].text_sha256
        );
    }

    #[test]
    #[should_panic(expected = "generations after zero must be self-loads")]
    fn task_admission_plan_rejects_a_second_entry() {
        let entry = admission_generation(TaskAdmissionSource::TaskEntry, 0x1000, 0x10);
        TaskAdmissionPlan::new(entry, [entry]);
    }

    #[test]
    fn task_admission_plan_identity_binds_generation_order_and_every_field_family() {
        let entry = admission_generation(TaskAdmissionSource::TaskEntry, 0x1000, 0x10);
        let a = admission_generation(TaskAdmissionSource::SelfLoad, 0x1000, 0x11);
        let b = admission_generation(TaskAdmissionSource::SelfLoad, 0x2000, 0x20);
        let baseline = TaskAdmissionPlan::new(entry, [a, b]);
        assert_eq!(baseline.sha256(), baseline.clone().sha256());
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [b, a]).sha256()
        );

        let mut changed = a;
        changed.text_address += 8;
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [changed, b]).sha256()
        );
        changed = a;
        changed.data_address += 8;
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [changed, b]).sha256()
        );
        changed = a;
        changed.text_sha256 = UcodeDigest::from_sha256([0x55; 32]);
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [changed, b]).sha256()
        );
        changed = a;
        changed.data.bytes += 8;
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [changed, b]).sha256()
        );
        changed = a;
        changed.data.sha256 = [0x66; 32];
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [changed, b]).sha256()
        );
        changed = a;
        changed.ucode = TaskAdmissionUcode::Other(7);
        assert_ne!(
            baseline.sha256(),
            TaskAdmissionPlan::new(entry, [changed, b]).sha256()
        );
    }

    #[test]
    fn task_admission_identity_cannot_collapse_f3dzex2_behavior_variants() {
        let baseline = admission_generation(TaskAdmissionSource::TaskEntry, 0x1000, 0x10);
        let plan = |variant| {
            let mut generation = baseline;
            generation.ucode = TaskAdmissionUcode::F3dzex2(variant);
            TaskAdmissionPlan::new(generation, [])
        };
        let h206 = plan(F3dzex2Variant::NoNFifo206H);
        let i208 = plan(F3dzex2Variant::NoNFifo208I);
        let j208 = plan(F3dzex2Variant::NoNFifo208J);

        assert_eq!(h206.entry().family(), UcodeId::F3dzex2);
        assert_eq!(
            h206.entry().ucode.f3dzex2_variant(),
            Some(F3dzex2Variant::NoNFifo206H)
        );
        assert_ne!(h206.sha256(), i208.sha256());
        assert_ne!(h206.sha256(), j208.sha256());
        assert_ne!(i208.sha256(), j208.sha256());
    }

    #[test]
    #[should_panic(expected = "F3DZEX2 task admission requires an exact typed variant")]
    fn broad_f3dzex2_family_cannot_construct_executable_admission() {
        let _ = TaskAdmissionUcode::from_family(UcodeId::F3dzex2);
    }

    #[test]
    fn geometry_profiles_roundtrip_every_public_wire_family() {
        for family in [
            GeometryWireFamily::Fast3d,
            GeometryWireFamily::F3dex,
            GeometryWireFamily::F3dlx,
            GeometryWireFamily::F3dlxRej,
            GeometryWireFamily::F3dex2,
            GeometryWireFamily::F3dex2NoN,
            GeometryWireFamily::F3dex2Rej,
            GeometryWireFamily::F3dlx2Rej,
            GeometryWireFamily::L3dex,
            GeometryWireFamily::L3dex2,
        ] {
            let profile = GeometryUcodeProfile::from_public_family(family);
            assert_eq!(profile.wire_family(), family);
            assert_eq!(profile.family(), family.ucode_id());
            assert_eq!(profile.f3dzex2_variant(), None);
            assert_eq!(profile.no_n(), family == GeometryWireFamily::F3dex2NoN);
            assert!(!profile.point_lighting());
            assert_eq!(
                GeometryUcodeProfile::from_admission_ucode(profile.admission_ucode()),
                Some(profile)
            );
        }
        assert_eq!(
            GeometryUcodeProfile::from_admission_ucode(TaskAdmissionUcode::S2dex2),
            None
        );
        assert_eq!(
            GeometryUcodeProfile::from_admission_ucode(TaskAdmissionUcode::Other(7)),
            None
        );
    }

    #[test]
    fn typed_f3dzex2_profiles_bind_non_and_point_lighting_capabilities() {
        for (variant, point_lighting) in [
            (F3dzex2Variant::NoNFifo206H, false),
            (F3dzex2Variant::NoNFifo208I, true),
            (F3dzex2Variant::NoNFifo208J, true),
        ] {
            let ucode = TaskAdmissionUcode::F3dzex2(variant);
            let profile = GeometryUcodeProfile::from_admission_ucode(ucode)
                .expect("typed F3DZEX2 identity is a geometry profile");
            assert_eq!(profile.wire_family(), GeometryWireFamily::F3dzex2);
            assert_eq!(profile.family(), UcodeId::F3dzex2);
            assert_eq!(profile.admission_ucode(), ucode);
            assert_eq!(profile.f3dzex2_variant(), Some(variant));
            assert!(profile.no_n());
            assert_eq!(profile.point_lighting(), point_lighting);
        }
    }

    #[test]
    #[should_panic(expected = "F3DZEX2 geometry profile requires an exact typed variant")]
    fn broad_f3dzex2_family_cannot_construct_a_geometry_profile() {
        let _ = GeometryUcodeProfile::from_public_family(GeometryWireFamily::F3dzex2);
    }

    #[test]
    fn geometry_admission_is_exact_and_family_typed() {
        let text = [0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = GeometryUcodeCatalog::default();
        let digest = catalog.admit_text_for(GeometryWireFamily::L3dex2, &text);
        assert_eq!(catalog.family(digest), Some(GeometryWireFamily::L3dex2));
        assert_eq!(
            catalog.require_text(&text).unwrap(),
            GeometryWireFamily::L3dex2
        );
        let profile = catalog.require_profile_text(&text).unwrap();
        assert_eq!(catalog.profile(digest), Some(profile));
        assert_eq!(profile.wire_family(), GeometryWireFamily::L3dex2);
        assert_eq!(profile.admission_ucode(), TaskAdmissionUcode::L3dex2);
        assert_eq!(catalog.supported_ucodes(), &[UcodeId::L3dex2]);
    }

    #[test]
    fn f3dzex2_branchw_index_domain_is_not_collapsed_to_public_f3dex2() {
        assert_eq!(GeometryWireFamily::F3dex2.cache_capacity(), 32);
        assert_eq!(GeometryWireFamily::F3dex2.max_vertex_load_count(), 32);
        assert_eq!(GeometryWireFamily::F3dzex2.cache_capacity(), 128);
        assert_eq!(GeometryWireFamily::F3dzex2.max_vertex_load_count(), 127);
    }

    #[test]
    fn exact_text_data_pair_cannot_be_replaced_by_text_only_identity() {
        let text = [0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = MicrocodeDataImageIdentity {
            bytes: 8,
            sha256: [0x44; 32],
        };
        let mut catalog = MicrocodePairCatalog::default();
        catalog.admit(UcodeId::F3dex2, Sha256::digest(text).into(), data);
        assert_eq!(catalog.identify(&text, data), Some(UcodeId::F3dex2));
        assert_eq!(
            catalog.identify(&text, MicrocodeDataImageIdentity { bytes: 9, ..data }),
            None
        );
    }

    #[test]
    fn exact_pair_may_retain_f3dzex2_diagnostic_identity_without_hle_admission() {
        let text = [0x7a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = MicrocodeDataImageIdentity {
            bytes: 8,
            sha256: [0x44; 32],
        };
        let mut pairs = MicrocodePairCatalog::default();
        pairs.admit(UcodeId::F3dzex2, Sha256::digest(text).into(), data);

        assert_eq!(pairs.identify(&text, data), Some(UcodeId::F3dzex2));
        assert!(GeometryUcodeCatalog::default()
            .supported_ucodes()
            .is_empty());
    }

    #[test]
    fn default_admission_methods_select_the_documented_modern_families() {
        let text = [0x21; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut geometry = GeometryUcodeCatalog::default();
        geometry.admit_text(&text);
        assert_eq!(
            geometry.require_text(&text).unwrap(),
            GeometryWireFamily::F3dex2
        );

        let mut objects = S2dexUcodeCatalog::default();
        objects.admit_text(&text);
        assert_eq!(
            objects.require_text(&text).unwrap(),
            S2dexWireFamily::S2dex2
        );
    }

    #[test]
    fn geometry_support_order_is_stable_across_admission_order() {
        let mut catalog = GeometryUcodeCatalog::default();
        for (seed, family) in [
            (1, GeometryWireFamily::L3dex2),
            (2, GeometryWireFamily::Fast3d),
            (3, GeometryWireFamily::F3dex2Rej),
            (4, GeometryWireFamily::F3dex),
        ] {
            catalog.admit_text_for(family, &[seed; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        }
        assert_eq!(
            catalog.supported_ucodes(),
            &[
                UcodeId::Fast3d,
                UcodeId::F3dex,
                UcodeId::F3dex2Rej,
                UcodeId::L3dex2,
            ]
        );
    }

    #[test]
    #[should_panic(expected = "F3DZEX2 HLE admission requires")]
    fn unpublished_f3dzex2_wire_cannot_be_admitted() {
        GeometryUcodeCatalog::default().admit_text_for(
            GeometryWireFamily::F3dzex2,
            &[0x7a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
    }

    #[test]
    fn s2dex_support_report_tracks_each_exact_admitted_family() {
        let mut catalog = S2dexUcodeCatalog::default();
        assert_eq!(catalog.supported_ucodes(), &[]);
        catalog.admit_text_for(
            S2dexWireFamily::S2dex2,
            &[0x52; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        assert_eq!(catalog.supported_ucodes(), &[UcodeId::S2dex2]);
        catalog.admit_text_for(
            S2dexWireFamily::S2dex,
            &[0x51; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        assert_eq!(
            catalog.supported_ucodes(),
            &[UcodeId::S2dex, UcodeId::S2dex2]
        );
    }

    #[test]
    fn rejected_s2dex_conflict_preserves_the_original_family() {
        let text = [0x62; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = S2dexUcodeCatalog::default();
        catalog.admit_text_for(S2dexWireFamily::S2dex, &text);
        let conflict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            catalog.admit_text_for(S2dexWireFamily::S2dex2, &text);
        }));
        assert!(conflict.is_err());
        assert_eq!(catalog.require_text(&text).unwrap(), S2dexWireFamily::S2dex);
    }

    #[test]
    fn rejected_pair_conflict_preserves_the_original_family() {
        let text = [0x63; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let digest = Sha256::digest(text).into();
        let data = MicrocodeDataImageIdentity {
            bytes: 16,
            sha256: [0x64; 32],
        };
        let mut catalog = MicrocodePairCatalog::default();
        catalog.admit(UcodeId::F3dex, digest, data);
        let conflict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            catalog.admit(UcodeId::S2dex, digest, data);
        }));
        assert!(conflict.is_err());
        assert_eq!(catalog.identify(&text, data), Some(UcodeId::F3dex));
    }

    #[test]
    #[should_panic(expected = "S2DEX microcode admission requires one complete 4 KiB IMEM image")]
    fn s2dex_text_admission_rejects_partial_imem_images() {
        S2dexUcodeCatalog::default().admit_text(&[0; 8]);
    }

    #[test]
    fn unadmitted_text_returns_its_exact_digest() {
        let text = [0x65; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let expected = UcodeDigest::from_text(&text).as_bytes();
        for error in [
            GeometryUcodeCatalog::default()
                .require_text(&text)
                .unwrap_err(),
            S2dexUcodeCatalog::default()
                .require_text(&text)
                .unwrap_err(),
        ] {
            match error {
                RenderError::RequiresLle { ucode_sha256 } => {
                    assert_eq!(ucode_sha256, expected);
                }
                other => panic!("unadmitted microcode returned {other}"),
            }
        }
    }
}
