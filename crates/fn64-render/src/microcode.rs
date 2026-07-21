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

use crate::{MicrocodeDataImageIdentity, RenderError, UcodeId};
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
            Self::F3dlxRej | Self::F3dex2Rej | Self::F3dlx2Rej => 64,
            Self::F3dex
            | Self::F3dlx
            | Self::F3dex2
            | Self::F3dex2NoN
            | Self::F3dzex2
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

    #[doc(hidden)]
    pub fn family(&self, digest: UcodeDigest) -> Option<GeometryWireFamily> {
        self.digests.get(&digest).copied()
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
        assert_eq!(catalog.supported_ucodes(), &[UcodeId::L3dex2]);
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
