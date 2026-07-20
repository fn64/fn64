//! A digest-typed geometry display-list decoder: enough opcodes to turn real
//! OoT-era display list (segmented vertex/matrix data, an MVP transform
//! stack, and G_TRI1/G_TRI2/G_QUAD triangle commands) into screen-space
//! filled polygons. Public no-op commands are explicit; reserved, malformed,
//! and unknown commands trap with their wire context so coverage is never
//! silently overstated.
//!
//! ## Provenance
//!
//! `Gfx` is the N64 SDK's public 64-bit-word display-list command
//! encoding: every `gsSP*`/`gsDP*` macro in the publicly published
//! `gbi.h` header (redistributed in countless public SDK-header mirrors
//! and referenced throughout N64 homebrew/modding documentation) packs to
//! exactly this two-`u32` shape -- opcode in the top byte of the first
//! word, remaining fields packed by the specific opcode. Every opcode byte
//! value and bit-field offset below is cited to the F3DEX_GBI_2 branch of
//! the public `gbi.h` (`ultra64/gbi.h`): `gDma1p`/`gDma2p` word packing
//! (gbi.h ~2046-2090), `gsSPVertex` (~2150), `__gsSP1Triangle_w1` (~2320,
//! F3DEX branch: indices in w0, each `v*2`), `gsSPMatrix` (~2106),
//! `gsMoveWd`/`gsSPSegment` (~2267/2578), `gsSPDisplayList` (~2177). This
//! module reads only the raw wire values, not any vendor SDK/microcode C
//! source -- the encoding is packaging-level ABI, the same standing as this
//! project's other public-ABI citations (`os_task.h`'s `OSTask_t`).
//!
//! ## Scope
//!
//! Interpreted (real effect): `G_VTX` (load transformed vertices into the
//! 32-slot cache), `G_MODIFYVTX` (all four public post-transform cache
//! fields), `G_TRI1`/`G_TRI2`/`G_QUAD` (triangles referencing loaded slots),
//! `G_MTX`/`G_POPMTX` (modelview/projection stack), `G_CULLDL` (clip-code
//! volume culling), `G_RDPHALF_1` + `G_BRANCH_Z` (screen-depth tail branch),
//! `G_LINE3D` (clipped variable-width lines), `G_MOVEWORD` (segment,
//! light-count/color, fog, and force-matrix writes), `G_DL` (call/jump into a nested display
//! list), `G_SETOTHERMODE_H/L` (RDP cycle/filter/dither/render/alpha/
//! coverage/depth/blender state), `G_SETBLENDCOLOR` (alpha-test threshold),
//! `G_SETSCISSOR` (raster clip rectangle), `G_SETCONVERT`,
//! `G_SETKEYR`/`G_SETKEYGB`, `G_SETCIMG`, `G_SETFILLCOLOR`,
//! fill-cycle `G_FILLRECT`, copy/one/two-cycle `G_TEXRECT`, normal-cycle
//! `G_TEXRECTFLIP`, `G_RDPFULLSYNC`, and
//! `G_ENDDL` (stop). Renderable work is returned as an ordered [`RenderOp`]
//! stream; the compatibility triangle-only view is derived from that stream.
//!
//! `G_DMA_IO` executes against persistent RSP memory; unsupported
//! move-word/move-memory subindices, the three reserved special opcodes, and
//! any unrecognized byte are malformed-command traps. Texture, lighting, RDP
//! other-mode, alpha compare, and the color-combiner and framebuffer-blender
//! inputs needed by OoT are decoded.
use fn64_render::{RenderError, UcodeId};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::{collections::BTreeMap, fmt::Write as _};

/// Exact identity of one complete 4 KiB microcode text image. HLE family
/// admission is content-addressed: a changed IMEM generation is not assumed
/// compatible merely because the task that loaded it began as F3DEX2.
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

/// Public command-envelope identity for the geometry HLE decoder.
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

    const fn is_line(self) -> bool {
        matches!(self, Self::L3dex | Self::L3dex2)
    }

    const fn name(self) -> &'static str {
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

    const fn cache_capacity(self) -> usize {
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

    const fn uses_legacy_polygon_wire(self) -> bool {
        matches!(self, Self::F3dex | Self::F3dlx | Self::F3dlxRej)
    }

    const fn is_legacy_loadable(self) -> bool {
        matches!(
            self,
            Self::F3dex | Self::F3dlx | Self::F3dlxRej | Self::L3dex
        )
    }

    const fn max_vertex_load_count(self) -> usize {
        match self {
            Self::F3dex2Rej | Self::F3dlx2Rej => 64,
            _ => 32,
        }
    }

    const fn is_reject(self) -> bool {
        matches!(self, Self::F3dlxRej | Self::F3dex2Rej | Self::F3dlx2Rej)
    }

    const fn has_unpublished_wire(self) -> bool {
        matches!(self, Self::F3dzex2)
    }
}

/// Exact public polygon/line-family text images admitted for geometry HLE.
/// Admission applies equally to task-entry images and compatible self-loads;
/// neither the task header nor a colliding opcode chooses the wire family.
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
            self.supported.sort_by_key(|ucode| match ucode {
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
            });
        }
    }

    /// Return the explicit wire family only when the exact text is admitted.
    pub fn require_text(&self, text: &[u8]) -> Result<GeometryWireFamily, RenderError> {
        let digest = UcodeDigest::from_text(text);
        self.family(digest).ok_or(RenderError::RequiresLle {
            ucode_sha256: digest.as_bytes(),
        })
    }

    fn family(&self, digest: UcodeDigest) -> Option<GeometryWireFamily> {
        self.digests.get(&digest).copied()
    }

    pub(crate) fn identify_text(
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

/// Source-compatible name for callers that admit only F3DEX2 through the
/// original default-family methods.
pub type F3dex2UcodeCatalog = GeometryUcodeCatalog;

// --- Opcode bytes: F3DEX_GBI_2 branch of the public ultra64/gbi.h ---
pub const G_VTX: u8 = 0x01;
pub const G_TRI1: u8 = 0x05;
pub const G_TRI2: u8 = 0x06;
pub const G_QUAD: u8 = 0x07;
pub const G_TEXTURE: u8 = 0xD7;
pub const G_POPMTX: u8 = 0xD8;
pub const G_GEOMETRYMODE: u8 = 0xD9;
pub const G_MTX: u8 = 0xDA;
pub const G_MOVEWORD: u8 = 0xDB;
pub const G_MOVEMEM: u8 = 0xDC;
pub const G_DL: u8 = 0xDE;
pub const G_ENDDL: u8 = 0xDF;
const G_NOOP: u8 = 0x00;
const G_SPNOOP: u8 = 0xE0;
const G_RDPHALF_2: u8 = 0xF1;

const LEGACY_G_SPNOOP: u8 = 0x00;
const L3DEX_G_MTX: u8 = 0x01;
const L3DEX_G_MOVEMEM: u8 = 0x03;
const L3DEX_G_VTX: u8 = 0x04;
const L3DEX_G_DL: u8 = 0x06;
const F3DEX_G_LOAD_UCODE: u8 = 0xaf;
const F3DEX_G_BRANCH_Z: u8 = 0xb0;
const F3DEX_G_TRI2: u8 = 0xb1;
const F3DEX_G_MODIFYVTX: u8 = 0xb2;
const LEGACY_G_RDPHALF_2: u8 = 0xb3;
const LEGACY_G_RDPHALF_1: u8 = 0xb4;
const L3DEX_G_LINE3D: u8 = 0xb5;
const L3DEX_G_CLEARGEOMETRYMODE: u8 = 0xb6;
const L3DEX_G_SETGEOMETRYMODE: u8 = 0xb7;
const L3DEX_G_ENDDL: u8 = 0xb8;
const L3DEX_G_SETOTHERMODE_L: u8 = 0xb9;
const L3DEX_G_SETOTHERMODE_H: u8 = 0xba;
const L3DEX_G_TEXTURE: u8 = 0xbb;
const L3DEX_G_MOVEWORD: u8 = 0xbc;
const L3DEX_G_POPMTX: u8 = 0xbd;
const LEGACY_G_CULLDL: u8 = 0xbe;
const L3DEX_G_TRI1: u8 = 0xbf;
const L3DEX_G_NOOP: u8 = 0xc0;

const LEGACY_G_MW_POINTS: u32 = 0x0c;
const LEGACY_G_CLIPPING: u32 = 0x0080_0000;
const LEGACY_G_SHADING_SMOOTH: u32 = 0x0000_0200;
const LEGACY_G_CULL_FRONT: u32 = 0x0000_1000;
const LEGACY_G_CULL_BACK: u32 = 0x0000_2000;

fn normalize_legacy_geometry_mode(family: GeometryWireFamily, word: u32) -> u32 {
    const SHARED: u32 = 0x0000_0001
        | 0x0000_0002
        | 0x0000_0004
        | 0x0001_0000
        | 0x0002_0000
        | 0x0004_0000
        | 0x0008_0000
        | 0x0010_0000;
    const PUBLIC: u32 = SHARED
        | LEGACY_G_SHADING_SMOOTH
        | LEGACY_G_CULL_FRONT
        | LEGACY_G_CULL_BACK
        | LEGACY_G_CLIPPING;
    assert_eq!(
        word & !PUBLIC,
        0,
        "{} geometry mode contains non-public legacy bits {:#010x}",
        family.name(),
        word & !PUBLIC
    );
    assert!(
        family != GeometryWireFamily::L3dex
            || word & (LEGACY_G_CULL_FRONT | LEGACY_G_CULL_BACK) == 0,
        "L3DEX does not support public polygon cull geometry modes"
    );
    assert!(
        !matches!(
            family,
            GeometryWireFamily::F3dlx | GeometryWireFamily::F3dlxRej
        ) || word & LEGACY_G_CULL_FRONT == 0,
        "{} does not support G_CULL_FRONT or G_CULL_BOTH",
        family.name()
    );
    assert!(
        family == GeometryWireFamily::F3dlx || word & LEGACY_G_CLIPPING == 0,
        "{} does not support the F3DLX-only G_CLIPPING toggle",
        family.name()
    );
    (word & SHARED)
        | if word & LEGACY_G_SHADING_SMOOTH != 0 {
            G_SHADING_SMOOTH
        } else {
            0
        }
        | if word & LEGACY_G_CULL_FRONT != 0 {
            G_CULL_FRONT
        } else {
            0
        }
        | if word & LEGACY_G_CULL_BACK != 0 {
            G_CULL_BACK
        } else {
            0
        }
        | (word & LEGACY_G_CLIPPING)
}

fn normalize_legacy_triangle_word(family: GeometryWireFamily, packed: u32) -> u32 {
    let slots = if family == GeometryWireFamily::Fast3d {
        let flag = ((packed >> 24) & 0xff) as usize;
        assert!(
            flag <= 2,
            "Fast3D G_TRI1 flat-shade flag {flag} is outside 0..=2"
        );
        let encoded = [
            ((packed >> 16) & 0xff) as usize,
            ((packed >> 8) & 0xff) as usize,
            (packed & 0xff) as usize,
        ];
        assert!(
            encoded
                .iter()
                .all(|value| value.is_multiple_of(10) && *value <= 150),
            "Fast3D triangle vertices must use public v*10 packing: {encoded:?}"
        );
        let mut slots = encoded.map(|value| (value / 10) as u32);
        slots.rotate_left(flag);
        slots
    } else {
        assert_eq!(
            packed >> 24,
            0,
            "{} triangle reserves packed bits 31..24",
            family.name()
        );
        let encoded = [
            ((packed >> 16) & 0xff) as usize,
            ((packed >> 8) & 0xff) as usize,
            (packed & 0xff) as usize,
        ];
        let max_encoded = family.cache_capacity() * 2 - 2;
        assert!(
            encoded
                .iter()
                .all(|value| value.is_multiple_of(2) && *value <= max_encoded),
            "{} triangle vertices must use public v*2 packing: {encoded:?}",
            family.name()
        );
        encoded.map(|value| (value / 2) as u32)
    };
    (slots[0] << 17) | (slots[1] << 9) | (slots[2] << 1)
}

/// Normalize only fully published base/F3DEX-envelope commands into the
/// already-evidenced F3DEX2 mechanisms. Unknown legacy bytes trap here before
/// they can collide with an unrelated F3DEX2 opcode.
fn normalize_geometry_command(
    family: GeometryWireFamily,
    wire_w0: u32,
    wire_w1: u32,
    command_pc: usize,
) -> (u32, u32) {
    assert!(
        !family.has_unpublished_wire(),
        "F3DZEX2 command decode requires an allowed-source wire specification before HLE admission"
    );
    if matches!(
        family,
        GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej
            | GeometryWireFamily::L3dex2
    ) {
        return (wire_w0, wire_w1);
    }
    let opcode = (wire_w0 >> 24) as u8;
    match opcode {
        LEGACY_G_SPNOOP => {
            assert_eq!(wire_w0, 0, "{} G_SPNOOP command word must be zero", family.name());
            assert_eq!(wire_w1, 0, "{} G_SPNOOP second word must be zero", family.name());
            (u32::from(G_SPNOOP) << 24, 0)
        }
        L3DEX_G_MTX => {
            let old_params = ((wire_w0 >> 16) & 0xff) as u8;
            assert_eq!(
                wire_w0 & 0xffff,
                64,
                "{} G_MTX must carry one 64-byte Mtx",
                family.name()
            );
            assert_eq!(
                old_params & !0x07,
                0,
                "{} G_MTX parameter {old_params:#04x} contains non-public bits",
                family.name()
            );
            let new_params = ((old_params & 0x01) << 2)
                | (old_params & 0x02)
                | ((old_params & 0x04) >> 2);
            let wire_index = new_params ^ 0x01;
            (
                (u32::from(G_MTX) << 24) | (7 << 19) | u32::from(wire_index),
                wire_w1,
            )
        }
        L3DEX_G_MOVEMEM => {
            let index = ((wire_w0 >> 16) & 0xff) as u8;
            let length = wire_w0 & 0xffff;
            assert_eq!(length, 16, "{} G_MOVEMEM must carry one 16-byte record", family.name());
            let (modern_index, ofs_div8) = match index {
                0x80 => (G_MV_VIEWPORT, 0),
                0x82 => (G_MV_LIGHT, 3),
                0x84 => (G_MV_LIGHT, 0),
                0x86..=0x94 if index.is_multiple_of(2) => {
                    let light = usize::from((index - 0x86) / 2) + 1;
                    (G_MV_LIGHT, (light + 1) * 3)
                }
                _ => crate::render_unsupported_panic(
                    "render.gbi.geometry.movemem",
                    format!(
                        "{} G_MOVEMEM index {index:#04x} is outside the admitted public viewport/look-at/light forms",
                        family.name()
                    ),
                ),
            };
            (
                (u32::from(G_MOVEMEM) << 24)
                    | (1 << 19)
                    | ((ofs_div8 as u32) << 8)
                    | u32::from(modern_index),
                wire_w1,
            )
        }
        L3DEX_G_VTX => {
            let parameter = ((wire_w0 >> 16) & 0xff) as usize;
            let length = wire_w0 & 0xffff;
            let (n, v0, limit) = if family == GeometryWireFamily::Fast3d {
                let n = (parameter >> 4) + 1;
                let v0 = parameter & 0x0f;
                assert_eq!(
                    length,
                    (n * VTX_STRIDE) as u32,
                    "Fast3D G_VTX length must be sizeof(Vtx)*n"
                );
                (n, v0, 16)
            } else {
                assert!(
                    parameter.is_multiple_of(2),
                    "{} G_VTX destination {parameter:#04x} is not public v0*2 packing",
                    family.name()
                );
                let v0 = parameter / 2;
                let n = ((length >> 10) & 0x3f) as usize;
                assert!(
                    (1..=32).contains(&n)
                        && length & 0x03ff == (n as u32 * VTX_STRIDE as u32 - 1),
                    "{} G_VTX length/count field {length:#06x} is not ((n<<10)|(16*n-1))",
                    family.name()
                );
                (n, v0, family.cache_capacity())
            };
            assert!(
                v0 + n <= limit,
                "{} G_VTX destination range {v0}..{} exceeds its {limit}-slot cache",
                family.name(),
                v0 + n
            );
            (
                (u32::from(G_VTX) << 24)
                    | ((n as u32) << 12)
                    | (((v0 + n) as u32) << 1),
                wire_w1,
            )
        }
        L3DEX_G_DL => {
            let parameter = (wire_w0 >> 16) & 0xff;
            assert!(
                matches!(parameter, 0 | 1) && wire_w0 & 0xffff == 0,
                "{} G_DL requires public push/nopush parameter and zero length: w0={wire_w0:#010x}",
                family.name()
            );
            ((u32::from(G_DL) << 24) | (parameter << 16), wire_w1)
        }
        L3DEX_G_LINE3D => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_LINE3D reserved first-word payload must be zero",
                family.name()
            );
            assert!(
                !family.uses_legacy_polygon_wire(),
                "{} polygon microcode does not execute G_LINE3D; load L3DEX explicitly",
                family.name()
            );
            let slots = if family == GeometryWireFamily::Fast3d {
                let flag = (wire_w1 >> 24) as usize;
                assert!(flag <= 1, "Fast3D G_LINE3D flat-shade flag must be 0 or 1");
                let encoded = [((wire_w1 >> 16) & 0xff) as usize, ((wire_w1 >> 8) & 0xff) as usize];
                assert!(
                    encoded.iter().all(|value| value.is_multiple_of(10) && *value <= 150),
                    "Fast3D G_LINE3D endpoints must use public v*10 packing: {encoded:?}"
                );
                let mut slots = [encoded[0] / 10, encoded[1] / 10];
                if flag == 1 {
                    slots.swap(0, 1);
                }
                slots
            } else {
                assert_eq!(wire_w1 >> 24, 0, "L3DEX G_LINE3D reserves w1[31:24]");
                let encoded = [((wire_w1 >> 16) & 0xff) as usize, ((wire_w1 >> 8) & 0xff) as usize];
                let max_encoded = family.cache_capacity() * 2 - 2;
                assert!(
                    encoded
                        .iter()
                        .all(|value| value.is_multiple_of(2) && *value <= max_encoded),
                    "L3DEX G_LINE3D endpoints must use public v*2 packing: {encoded:?}"
                );
                [encoded[0] / 2, encoded[1] / 2]
            };
            (
                (u32::from(G_LINE3D) << 24)
                    | ((slots[0] as u32 * 2) << 16)
                    | ((slots[1] as u32 * 2) << 8)
                    | (wire_w1 & 0xff),
                0,
            )
        }
        LEGACY_G_CULLDL => {
            let (start, end) = if family == GeometryWireFamily::Fast3d {
                let start_bytes = (wire_w0 & 0x00ff_ffff) as usize;
                let end_exclusive_bytes = wire_w1 as usize;
                assert!(
                    start_bytes.is_multiple_of(40) && end_exclusive_bytes.is_multiple_of(40),
                    "Fast3D G_CULLDL indices must use public 40-byte vertex-record offsets"
                );
                let start = start_bytes / 40;
                let end_exclusive = end_exclusive_bytes / 40;
                assert!(
                    start < end_exclusive && end_exclusive <= 16,
                    "Fast3D G_CULLDL range {start}..{end_exclusive} exceeds its 16-slot cache"
                );
                (start, end_exclusive - 1)
            } else {
                assert_eq!(wire_w0 & 0x00ff_0000, 0, "{} G_CULLDL reserves w0[23:16]", family.name());
                assert_eq!(wire_w1 & 0xffff_0000, 0, "{} G_CULLDL reserves w1[31:16]", family.name());
                let encoded = [(wire_w0 & 0xffff) as usize, (wire_w1 & 0xffff) as usize];
                let max_encoded = family.cache_capacity() * 2 - 2;
                assert!(
                    encoded
                        .iter()
                        .all(|value| value.is_multiple_of(2) && *value <= max_encoded),
                    "{} G_CULLDL indices must use public v*2 packing: {encoded:?}",
                    family.name()
                );
                (encoded[0] / 2, encoded[1] / 2)
            };
            (
                (u32::from(G_CULLDL) << 24) | ((start as u32) * 2),
                (end as u32) * 2,
            )
        }
        L3DEX_G_TRI1 if family != GeometryWireFamily::L3dex => {
            let normalized = normalize_legacy_triangle_word(family, wire_w1);
            assert_eq!(wire_w0 & 0x00ff_ffff, 0, "{} G_TRI1 reserves its first-word payload", family.name());
            ((u32::from(G_TRI1) << 24) | normalized, 0)
        }
        F3DEX_G_TRI2 if family.uses_legacy_polygon_wire() => {
            let first = normalize_legacy_triangle_word(family, wire_w0 & 0x00ff_ffff);
            let second = normalize_legacy_triangle_word(family, wire_w1);
            ((u32::from(G_TRI2) << 24) | first, second)
        }
        F3DEX_G_MODIFYVTX if family.uses_legacy_polygon_wire() => {
            let where_field = (wire_w0 >> 16) & 0xff;
            let encoded = (wire_w0 & 0xffff) as usize;
            assert!(
                encoded.is_multiple_of(2) && encoded < family.cache_capacity() * 2,
                "{} G_MODIFYVTX cache index must use public v*2 packing",
                family.name()
            );
            (
                (u32::from(G_MODIFYVTX) << 24) | (where_field << 16) | encoded as u32,
                wire_w1,
            )
        }
        F3DEX_G_BRANCH_Z if family.uses_legacy_polygon_wire() => {
            ((u32::from(G_BRANCH_Z) << 24) | (wire_w0 & 0x00ff_ffff), wire_w1)
        }
        F3DEX_G_LOAD_UCODE if family.is_legacy_loadable() => {
            ((u32::from(G_LOAD_UCODE) << 24) | (wire_w0 & 0x00ff_ffff), wire_w1)
        }
        LEGACY_G_RDPHALF_1 => (u32::from(G_RDPHALF_1) << 24, wire_w1),
        LEGACY_G_RDPHALF_2 => (u32::from(G_RDPHALF_2) << 24, wire_w1),
        L3DEX_G_CLEARGEOMETRYMODE => {
            assert_eq!(wire_w0 & 0x00ff_ffff, 0, "{} G_CLEARGEOMETRYMODE payload must be zero", family.name());
            let clear = normalize_legacy_geometry_mode(family, wire_w1);
            ((u32::from(G_GEOMETRYMODE) << 24) | ((!clear) & 0x00ff_ffff), 0)
        }
        L3DEX_G_SETGEOMETRYMODE => {
            assert_eq!(wire_w0 & 0x00ff_ffff, 0, "{} G_SETGEOMETRYMODE payload must be zero", family.name());
            let set = normalize_legacy_geometry_mode(family, wire_w1);
            ((u32::from(G_GEOMETRYMODE) << 24) | 0x00ff_ffff, set)
        }
        L3DEX_G_ENDDL => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_ENDDL reserved first-word payload must be zero",
                family.name()
            );
            assert_eq!(wire_w1, 0, "{} G_ENDDL reserved second word must be zero", family.name());
            (u32::from(G_ENDDL) << 24, 0)
        }
        L3DEX_G_SETOTHERMODE_L | L3DEX_G_SETOTHERMODE_H => {
            let shift = (wire_w0 >> 8) & 0xff;
            let length = wire_w0 & 0xff;
            assert!(
                (1..=32).contains(&length) && shift + length <= 32,
                "{} G_SETOTHERMODE range shift={shift} length={length} is outside one 32-bit word",
                family.name()
            );
            let normalized_opcode = if opcode == L3DEX_G_SETOTHERMODE_H {
                G_SETOTHERMODE_H
            } else {
                G_SETOTHERMODE_L
            };
            (
                (u32::from(normalized_opcode) << 24)
                    | ((32 - shift - length) << 8)
                    | (length - 1),
                wire_w1,
            )
        }
        L3DEX_G_TEXTURE => {
            let on = wire_w0 & 0xff;
            assert!(matches!(on, 0 | 1), "{} G_TEXTURE on={on} is outside G_OFF/G_ON", family.name());
            (
                (u32::from(G_TEXTURE) << 24)
                    | (wire_w0 & 0x00ff_ff00)
                    | (on << 1),
                wire_w1,
            )
        }
        L3DEX_G_MOVEWORD => {
            let offset = (wire_w0 >> 8) & 0xffff;
            let index = wire_w0 & 0xff;
            if index == LEGACY_G_MW_POINTS {
                let slot = offset / 40;
                let where_field = offset % 40;
                let limit = family.cache_capacity();
                assert!(
                    slot < limit as u32,
                    "{} G_MW_POINTS slot {slot} exceeds its {limit}-slot cache",
                    family.name()
                );
                assert!(
                    matches!(where_field, 0x10 | 0x14 | 0x18 | 0x1c),
                    "{} G_MW_POINTS destination {where_field:#04x} is not a public post-transform field",
                    family.name()
                );
                return (
                    (u32::from(G_MODIFYVTX) << 24)
                        | (where_field << 16)
                        | (slot * 2),
                    wire_w1,
                );
            }
            let (offset, data) = if index == u32::from(G_MW_NUMLIGHT) {
                assert_eq!(offset, 0, "{} G_MW_NUMLIGHT offset must be zero", family.name());
                assert!(
                    wire_w1 & 0x8000_0000 != 0 && (wire_w1 & 0x7fff_ffff).is_multiple_of(32),
                    "{} G_MW_NUMLIGHT data is not public ((n+1)*32)|0x80000000 packing",
                    family.name()
                );
                let n = (wire_w1 & 0x7fff_ffff) / 32;
                assert!((2..=8).contains(&n), "{} G_MW_NUMLIGHT count is outside 1..=7", family.name());
                (offset, (n - 1) * 24)
            } else if index == u32::from(G_MW_LIGHTCOL) {
                assert!(offset.is_multiple_of(4), "{} G_MW_LIGHTCOL offset must be word aligned", family.name());
                let light = offset / 0x20;
                let copy = offset % 0x20;
                assert!(light < 8 && matches!(copy, 0 | 4), "{} G_MW_LIGHTCOL offset {offset:#06x} is outside public light colors", family.name());
                (light * 0x18 + copy, wire_w1)
            } else {
                (offset, wire_w1)
            };
            (
                (u32::from(G_MOVEWORD) << 24) | (index << 16) | offset,
                data,
            )
        }
        L3DEX_G_POPMTX => {
            assert_eq!(wire_w0 & 0x00ff_ffff, 0, "{} G_POPMTX payload must be zero", family.name());
            assert_eq!(wire_w1, 0, "{} G_POPMTX supports only G_MTX_MODELVIEW", family.name());
            (u32::from(G_POPMTX) << 24, 64)
        }
        L3DEX_G_NOOP => {
            assert_eq!(wire_w0 & 0x00ff_ffff, 0, "{} G_NOOP payload must be zero", family.name());
            assert_eq!(wire_w1, 0, "{} G_NOOP second word must be zero", family.name());
            (u32::from(G_NOOP) << 24, 0)
        }
        0xe4..=0xff => (wire_w0, wire_w1),
        _ => crate::render_unsupported_panic(
            "render.gbi.geometry.command",
            format!(
                "unsupported {} command byte {opcode:#04x} at RDRAM {command_pc:#010x}: w0={wire_w0:#010x} w1={wire_w1:#010x}",
                family.name()
            ),
        ),
    }
}

fn consume_line_triangle_noop(family: GeometryWireFamily, wire_w0: u32, wire_w1: u32) -> bool {
    let wire_opcode = (wire_w0 >> 24) as u8;
    let packed = match family {
        GeometryWireFamily::F3dex2 => return false,
        GeometryWireFamily::L3dex2 if wire_opcode == G_TRI1 => {
            assert_eq!(wire_w1, 0, "L3DEX2 G_TRI1 NOOP reserves its second word");
            wire_w0 & 0x00ff_ffff
        }
        GeometryWireFamily::L3dex if wire_opcode == L3DEX_G_TRI1 => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "L3DEX G_TRI1 NOOP reserves its first-word payload"
            );
            assert_eq!(wire_w1 >> 24, 0, "L3DEX G_TRI1 NOOP reserves w1[31:24]");
            wire_w1 & 0x00ff_ffff
        }
        _ => return false,
    };
    let encoded = [
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
    ];
    assert!(
        encoded
            .iter()
            .all(|value| value.is_multiple_of(2) && *value <= 62),
        "line-microcode G_TRI1 NOOP vertices must use public v*2 packing: {encoded:?}"
    );
    true
}

/// `G_MW_SEGMENT` (gbi.h:1212) -- the `G_MOVEWORD` index that writes the
/// segment base-address table used to resolve segmented pointers.
const G_MW_SEGMENT: u16 = 0x06;

/// `G_MV_VIEWPORT` (gbi.h) -- the `G_MOVEMEM` index that DMAs a `Vp`
/// (viewport scale/translate) struct into RSP state (F3DEX2-CONCEPTS.md
/// §1.4/§3.5).
const G_MV_VIEWPORT: u8 = 8;
/// Destination used by the first half of public F3DEX2 `gSPForceMatrix` to
/// DMA an already-concatenated model/projection matrix into RSP state.
const G_MV_MATRIX: u8 = 14;

// `gSPModifyVertex` destinations from the public F3DEX2 manual. These are
// final post-transform cache fields; the command does not re-run lighting or
// matrix projection.
const G_MWO_POINT_RGBA: u8 = 0x10;
const G_MWO_POINT_ST: u8 = 0x14;
const G_MWO_POINT_XYSCREEN: u8 = 0x18;
const G_MWO_POINT_ZSCREEN: u8 = 0x1C;

// --- F3DEX2 geometry-mode bits (F3DEX2-CONCEPTS.md §2.4) -----------------
/// Cull front-facing triangles.
const G_CULL_FRONT: u32 = 0x0000_0200;
/// Cull back-facing triangles (the common case).
const G_CULL_BACK: u32 = 0x0000_0400;
/// Enable vertex lighting. When set, a vertex's `cn[0..3]` bytes are a signed
/// s8 NORMAL (x,y,z), not an RGB color -- the vertex color is COMPUTED from
/// the loaded lights (ambient + per-directional N·L·color) instead of taken
/// from `cn` (`F3DEX2-CONCEPTS.md` §2.4; OoT gbi.h `G_LIGHTING`). Reading the
/// normal bytes as a flat color (the pre-lighting path) produced the
/// characteristic "rainbow fan" -- signed normal components reinterpreted as
/// unsigned color channels.
const G_LIGHTING: u32 = 0x0002_0000;
/// Generate the vertex alpha coordinate from projected depth and the current
/// signed fog multiplier/offset instead of preserving the source vertex alpha.
const G_FOG: u32 = 0x0001_0000;
/// Generate texture S/T from the signed vertex normal projected onto the
/// two screen-space directions loaded by `gSPLookAt`.
const G_TEXTURE_GEN: u32 = 0x0004_0000;
/// Select the inverse-cosine texture-generation mapping. Public F3DEX2 uses
/// this together with [`G_TEXTURE_GEN`]; on its own it does not consume the
/// vertex normal or replace explicit texture coordinates.
const G_TEXTURE_GEN_LINEAR: u32 = 0x0008_0000;
/// Interpolate endpoint shade attributes instead of using the first encoded
/// endpoint selected by the line command's flat-shading flag.
const G_SHADING_SMOOTH: u32 = 0x0020_0000;

// --- F3DEX2 lighting: G_MOVEMEM/G_MOVEWORD indices + Light layout --------
/// `G_MV_LIGHT` (OoT gbi.h:1169) -- the `G_MOVEMEM` index that DMAs a `Light`
/// struct (diffuse color + direction, or an ambient color) into an RSP light
/// slot. F3DEX2 `gsSPLight` (gbi.h:2911) encodes `idx = G_MV_LIGHT` in the
/// w0 low byte and `ofs = n*24 + 24` (÷8 in the wire) in `field(w0,8,8)`.
const G_MV_LIGHT: u8 = 0x0a;
/// `G_MW_NUMLIGHT` (OoT gbi.h:1210) -- the `G_MOVEWORD` index that sets the
/// directional-light count. F3DEX2 `gsSPNumLights` (gbi.h:2887) writes
/// `NUML(n) = n*24` as the data word, so `numDirectional = w1 / 24`. The
/// AMBIENT light is the slot AFTER the directional ones (gbi.h:2902 note:
/// "the highest numbered light is always the ambient light").
const G_MW_NUMLIGHT: u16 = 0x02;
/// Public `gSPClipRatio` state block. Four writes select the negative/positive
/// X/Y clipping rectangle coefficients independently.
const G_MW_CLIP: u16 = 0x04;
const G_MWO_CLIP_RNX: u16 = 0x04;
const G_MWO_CLIP_RNY: u16 = 0x0c;
const G_MWO_CLIP_RPX: u16 = 0x14;
const G_MWO_CLIP_RPY: u16 = 0x1c;
/// `G_MW_FOG` packs signed `fm`/`fo` factors in the high/low halfwords.
const G_MW_FOG: u16 = 0x08;
/// `G_MW_LIGHTCOL` updates one of the two RGB copies in a light slot without
/// changing its direction. Public F3DEX2 `gbi.h` assigns each light a 24-byte
/// DMEM stride and exposes word offsets 0/4 within that stride.
const G_MW_LIGHTCOL: u16 = 0x0a;
/// Second half of public F3DEX2 `gSPForceMatrix`. The header macro writes
/// `0x0001_0000` at offset zero after the `G_MV_MATRIX` DMA.
const G_MW_FORCEMTX: u16 = 0x0c;
/// Public `.16` perspective-normalization scale written at offset zero.
const G_MW_PERSPNORM: u16 = 0x0e;
/// One `Light_t` on the wire is 16 bytes (OoT gbi.h:1311 -- `col[3]`, pad,
/// `colc[3]`, pad, `dir[3]`, pad), padded to a 16-byte `Light` union.
const LIGHT_STRIDE: usize = 16;
/// Max simultaneous lights F3DEX2 supports (7 directional + 1 ambient).
const MAX_LIGHTS: usize = 8;

// --- Additional F3DEX2 opcode bytes. Reserved/unsupported encodings are
// named so their loud traps report the public command identity.
const G_MODIFYVTX: u8 = 0x02;
const G_CULLDL: u8 = 0x03;
const G_BRANCH_Z: u8 = 0x04;
const G_LINE3D: u8 = 0x08;
const G_SPECIAL_1: u8 = 0xD5;
const G_SPECIAL_2: u8 = 0xD4;
const G_SPECIAL_3: u8 = 0xD3;
const G_DMA_IO: u8 = 0xD6;
const G_LOAD_UCODE: u8 = 0xDD;
/// Public `OSTask` guidance fixes task microcode text at `SP_UCODE_SIZE`;
/// the documented value is one complete 4 KiB IMEM bank. `gSPLoadUcodeEx`
/// carries only the data-section size because the text transfer has this
/// fixed size.
const SP_UCODE_SIZE: usize = fn64_runtime::RSP_MEMORY_BANK_SIZE;
/// `G_TEXRECT` / `G_TEXRECTFLIP` (gbi.h:126-127). The raw RDP command is two
/// 64-bit words (16 bytes). `gSPTextureRectangle` wraps the second word in two
/// family-specific `G_RDPHALF_*` commands, making three `Gfx` entries. The
/// ordered decoder consumes either public form; the reference executor
/// implements non-flipped copy plus one/two-cycle normal and flipped
/// combiner/blender paths. Flipped copy remains a named gap.
const G_TEXRECT: u8 = 0xE4;
const G_TEXRECTFLIP: u8 = 0xE5;
/// F3DEX2 staging word used by compound commands. `G_BRANCH_Z` consumes it as
/// the conditional branch target; `G_LOAD_UCODE` uses the same wire opcode
/// for its data address.
const G_RDPHALF_1: u8 = 0xE1;

/// Decode the second half of a public texture-rectangle command. Public
/// `gbi.h` exposes two wire forms: `gDPTextureRectangle` appends one raw RDP
/// word, while the display-list-safe `gSPTextureRectangle` wraps that word in
/// the family's `G_RDPHALF_1`/`G_RDPHALF_2` command envelope. The exact task
/// text selects which envelope is legal; opcode inspection never selects a
/// microcode family.
fn decode_texture_rectangle_continuation(
    rdram: &[u8],
    pc: usize,
    family: GeometryWireFamily,
    raw_rdp: bool,
    opcode: u8,
) -> (u32, u32, usize) {
    let direct_end = pc.checked_add(8).unwrap_or_else(|| {
        panic!(
            "{} continuation PC {pc:#010x} overflows the host address space",
            opcode_name(opcode)
        )
    });
    assert!(
        direct_end <= rdram.len(),
        "{} is truncated at RDRAM {pc:#010x}: need 8 continuation bytes, rdram_bytes={}",
        opcode_name(opcode),
        rdram.len()
    );
    let first_w0 = read_u32(rdram, pc);
    let first_w1 = read_u32(rdram, pc + 4);
    if raw_rdp {
        return (first_w0, first_w1, 8);
    }

    let modern = matches!(
        family,
        GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej
            | GeometryWireFamily::L3dex2
    );
    let (half_1, half_2) = if modern {
        (G_RDPHALF_1, G_RDPHALF_2)
    } else {
        (LEGACY_G_RDPHALF_1, LEGACY_G_RDPHALF_2)
    };
    let known_half_1 = [G_RDPHALF_1, LEGACY_G_RDPHALF_1]
        .into_iter()
        .find(|candidate| first_w0 == u32::from(*candidate) << 24);
    let Some(actual_half_1) = known_half_1 else {
        return (first_w0, first_w1, 8);
    };
    assert_eq!(
        actual_half_1,
        half_1,
        "{} {} continuation uses the wrong-family G_RDPHALF_1 opcode {actual_half_1:#04x}",
        family.name(),
        opcode_name(opcode)
    );

    let envelope_end = pc.checked_add(16).unwrap_or_else(|| {
        panic!(
            "{} {} continuation envelope at {pc:#010x} overflows the host address space",
            family.name(),
            opcode_name(opcode)
        )
    });
    assert!(
        envelope_end <= rdram.len(),
        "{} {} continuation envelope is truncated at RDRAM {pc:#010x}: need 16 bytes, rdram_bytes={}",
        family.name(),
        opcode_name(opcode),
        rdram.len()
    );
    let second_w0 = read_u32(rdram, pc + 8);
    let second_w1 = read_u32(rdram, pc + 12);
    assert_eq!(
        second_w0,
        u32::from(half_2) << 24,
        "{} {} G_RDPHALF_2 continuation must be opcode {half_2:#04x} with zero reserved payload",
        family.name(),
        opcode_name(opcode)
    );
    (first_w1, second_w1, 16)
}
/// RDP synchronization and untextured rectangle commands (public gbi.h
/// command IDs). Unlike RSP geometry commands, these must remain ordered with
/// triangles because they mutate or commit the active color image.
const G_RDPLOADSYNC: u8 = 0xE6;
const G_RDPPIPESYNC: u8 = 0xE7;
const G_RDPTILESYNC: u8 = 0xE8;
const G_RDPFULLSYNC: u8 = 0xE9;
const G_SETOTHERMODE_L: u8 = 0xE2;
const G_SETOTHERMODE_H: u8 = 0xE3;
/// Full RDP other-mode write (`gsDPSetOtherMode`; gbi.h:3724-3737).
const G_RDPSETOTHERMODE: u8 = 0xEF;
const G_SETSCISSOR: u8 = 0xED;
const G_SETCONVERT: u8 = 0xEC;
const G_SETKEYR: u8 = 0xEB;
const G_SETKEYGB: u8 = 0xEA;
const G_SETPRIMDEPTH: u8 = 0xEE;
const G_LOADTLUT: u8 = 0xF0;
const G_SETTILESIZE: u8 = 0xF2;
const G_LOADBLOCK: u8 = 0xF3;
const G_LOADTILE: u8 = 0xF4;
const G_SETTILE: u8 = 0xF5;
const G_FILLRECT: u8 = 0xF6;
const G_SETFILLCOLOR: u8 = 0xF7;
const G_SETFOGCOLOR: u8 = 0xF8;
const G_SETBLENDCOLOR: u8 = 0xF9;
const G_SETPRIMCOLOR: u8 = 0xFA;
const G_SETENVCOLOR: u8 = 0xFB;
const G_SETCOMBINE: u8 = 0xFC;
const G_SETTIMG: u8 = 0xFD;
const G_SETZIMG: u8 = 0xFE;
const G_SETCIMG: u8 = 0xFF;

/// One decoded vertex in screen space (after MVP + viewport if a transform
/// was active, or raw `ob` coords if no matrix was loaded -- see
/// `decode_display_list`) plus a flat RGBA color, matching the
/// position+color fields of the SDK's public `Vtx` union.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    /// Screen-space depth (mapped NDC-z through the viewport, nearer =
    /// smaller). Used by the z-buffer in `raster.rs`; 0.0 for the raw
    /// no-transform reference-fixture path (where all geometry is coplanar).
    pub z: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    /// Texture S/T coordinates in texels: the raw `Vtx` `tc[2]` S10.5
    /// fixed-point value multiplied by the `G_TEXTURE` S/T scale, then
    /// converted from the S10.5 encoding to texels (÷32). Only meaningful
    /// when the emitting triangle carries a `texture`; the rasterizer
    /// interpolates these per-pixel to address the decoded texel buffer
    /// (`F3DEX2-CONCEPTS.md` §5). 0.0 on the untextured/reference path.
    pub s: f32,
    pub t: f32,
    /// The homogeneous clip-space `w` this vertex was divided by (before the
    /// perspective divide). `w <= 0` means the vertex is AT or BEHIND the
    /// camera's near plane -- projecting it divides by a non-positive number
    /// and flings it to the opposite side of the screen, which is the "fan/
    /// bowtie from a central point" artifact. A triangle with any such vertex
    /// is dropped (coarse near-plane cull, see `behind_near_plane`) rather
    /// than drawn as a giant wrong-side polygon. `1.0` on the raw/reference
    /// path (no projection, everything in front).
    pub w: f32,
    /// Raw unsigned 16.16 screen-depth value retained for `G_BRANCH_Z`.
    /// Keeping this beside the display `z` prevents the conditional command
    /// from reconstructing a fixed-point comparison through host float.
    pub z_screen: u32,
    /// Six homogeneous viewing-volume side bits maintained when the vertex is
    /// transformed. `G_CULLDL` ANDs these codes across its inclusive range;
    /// a shared nonzero side means the complete bounding volume is outside.
    pub clip_code: u8,
    /// Homogeneous clip position retained for line clipping. Post-transform
    /// XY/Z modification invalidates this value because the public command
    /// supplies only final screen coordinates, not a reconstructable clip W.
    pub clip_position: Option<[f32; 4]>,
}

const CLIP_NEG_X: u8 = 1 << 0;
const CLIP_POS_X: u8 = 1 << 1;
const CLIP_NEG_Y: u8 = 1 << 2;
const CLIP_POS_Y: u8 = 1 << 3;
const CLIP_NEG_Z: u8 = 1 << 4;
const CLIP_POS_Z: u8 = 1 << 5;

/// Screen-space back/front-face culling selector, derived from the F3DEX2
/// `G_GEOMETRYMODE` `G_CULL_FRONT`/`G_CULL_BACK` bits
/// (`F3DEX2-CONCEPTS.md` §2.4). The rasterizer (`raster.rs`) applies it by
/// the sign of a triangle's screen-space signed area.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CullMode {
    /// No culling (both faces drawn).
    #[default]
    None,
    /// Cull back faces (`G_CULL_BACK`) -- the common OoT case.
    Back,
    /// Cull front faces (`G_CULL_FRONT`).
    Front,
    /// Cull both (`G_CULL_BOTH`) -- draws nothing.
    Both,
}

/// RDP cycle type from other-mode high bits 20..21 (`G_MDSFT_CYCLETYPE`).
/// Public `gbi.h` defines the four values at lines 527-531; RT64 exposes the
/// same masked field in `shared/rt64_other_mode.h:26-28`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CycleType {
    #[default]
    OneCycle,
    TwoCycle,
    Copy,
    Fill,
}

/// RDP texture filter from other-mode high bits 12..13
/// (`G_MDSFT_TEXTFILT`; public `gbi.h:514,551-554`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TextureFilter {
    #[default]
    Point,
    Reserved,
    Bilinear,
    Average,
}

/// RGB dither selector from other-mode high bits 6..7
/// (`G_MDSFT_RGBDITHER`; public `gbi.h:510,565-571`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RgbDither {
    #[default]
    MagicSquare,
    Bayer,
    Noise,
    Disabled,
}

/// Alpha dither selector from other-mode high bits 4..5
/// (`G_MDSFT_ALPHADITHER`; public `gbi.h:509,578-582`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AlphaDither {
    #[default]
    Pattern,
    InversePattern,
    Noise,
    Disabled,
}

/// Alpha-compare mode from other-mode low bits 0..1. The public constants
/// are `G_AC_NONE=0`, `G_AC_THRESHOLD=1`, and `G_AC_DITHER=3`
/// (`gbi.h:500,584-587`); value 2 is reserved.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AlphaCompare {
    #[default]
    None,
    Threshold,
    Reserved,
    Dither,
}

/// Coverage destination from render-mode bits 8..9
/// (`CVG_DST_*`, public `gbi.h:599-602`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CoverageDestination {
    #[default]
    Clamp,
    Wrap,
    Full,
    Save,
}

/// Z-mode from render-mode bits 10..11 (`ZMODE_*`, public
/// `gbi.h:603-606`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DepthMode {
    #[default]
    Opaque,
    Interpenetrating,
    Translucent,
    Decal,
}

/// The four two-bit blender selectors for one RDP cycle. Their positions are
/// the public `GBL_c1`/`GBL_c2` packing contract (`gbi.h:624-627`). Keeping
/// selectors as wire values avoids coupling this task to the separate color-
/// combiner implementation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlenderCycle {
    pub color_a: u8,
    pub alpha_a: u8,
    pub color_b: u8,
    pub alpha_b: u8,
}

/// The two RDP other-mode words plus the one color-register component alpha
/// comparison needs. F3DEX2 updates arbitrary bit ranges of H/L, so retaining
/// the raw words is the smallest merge-friendly representation; typed accessors
/// expose every render field this rasterizer or a future backend consumes.
///
/// Sources: public OoT `include/ultra64/gbi.h:497-627` (field shifts, values,
/// coverage/Z/blender packing), `gbi.h:3353-3369` (F3DEX2 partial-update wire
/// encoding), RT64 `shared/rt64_other_mode.h:14-101` (H/L field structure),
/// and RT64 `hle/rt64_rsp.cpp:1026-1037` (masked partial updates).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OtherMode {
    high: u32,
    low: u32,
    /// `G_SETBLENDCOLOR.a`, used by `G_AC_THRESHOLD` (RT64
    /// `shaders/RasterPS.hlsl:209-211`). Kept here, rather than adding prim/env
    /// color state, so the independently landing combiner remains isolated.
    pub blend_color_alpha: u8,
}

impl Default for OtherMode {
    fn default() -> Self {
        Self {
            // RT64's F3DEX2 reset state (`hle/rt64_rsp.cpp:88-89`). Low=0
            // means alpha compare off until the display list enables it.
            high: 0x0008_0cff,
            low: 0,
            blend_color_alpha: 0,
        }
    }
}

/// One semantic RGB input to the RDP color-combiner equation
/// `(A - B) * C + D`.
///
/// The raw numeric selector is position-dependent: selector `6`, for
/// example, means ONE in input A/D but KEY_CENTER/KEY_SCALE in B/C. The
/// decoder therefore resolves the wire value to this semantic enum at
/// `G_SETCOMBINE` time. Source values and position-specific meanings are
/// from OoT's public `ultra64/gbi.h:383-404` and RT64's MIT
/// `shared/rt64_color_combiner.h:59-151`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorSource {
    Combined,
    Texel0,
    Texel1,
    Primitive,
    Shade,
    Environment,
    KeyCenter,
    KeyScale,
    CombinedAlpha,
    Texel0Alpha,
    Texel1Alpha,
    PrimitiveAlpha,
    ShadeAlpha,
    EnvironmentAlpha,
    LodFraction,
    PrimLodFraction,
    Noise,
    K4,
    K5,
    One,
    Zero,
}

/// One semantic alpha input to the RDP color-combiner equation.
/// Selector values come from public `gbi.h:406-416`; the distinct C-input
/// mapping (where zero selects LOD fraction) is corroborated by RT64's MIT
/// `shared/rt64_color_combiner.h:153-193`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AlphaSource {
    Combined,
    Texel0,
    Texel1,
    Primitive,
    Shade,
    Environment,
    LodFraction,
    PrimLodFraction,
    One,
    Zero,
}

/// The eight selectors for one RDP combiner cycle: four RGB and four alpha.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CombinerCycle {
    pub rgb: [ColorSource; 4],
    pub alpha: [AlphaSource; 4],
}

/// Both cycles programmed by one `G_SETCOMBINE` command.
///
/// Bit locations are the public `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1`
/// packing macros (`ultra64/gbi.h:3543-3565`) and match RT64's MIT parse
/// helpers (`shared/rt64_color_combiner.h:195-240`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CombinerMode {
    pub cycles: [CombinerCycle; 2],
}

impl CombinerMode {
    fn decode(w0: u32, w1: u32) -> Self {
        CombinerMode {
            cycles: [
                CombinerCycle {
                    rgb: [
                        decode_color_a((w0 >> 20) & 0x0f),
                        decode_color_b((w1 >> 28) & 0x0f),
                        decode_color_c((w0 >> 15) & 0x1f),
                        decode_color_d((w1 >> 15) & 0x07),
                    ],
                    alpha: [
                        decode_alpha_abd((w0 >> 12) & 0x07),
                        decode_alpha_abd((w1 >> 12) & 0x07),
                        decode_alpha_c((w0 >> 9) & 0x07),
                        decode_alpha_abd((w1 >> 9) & 0x07),
                    ],
                },
                CombinerCycle {
                    rgb: [
                        decode_color_a((w0 >> 5) & 0x0f),
                        decode_color_b((w1 >> 24) & 0x0f),
                        decode_color_c(w0 & 0x1f),
                        decode_color_d((w1 >> 6) & 0x07),
                    ],
                    alpha: [
                        decode_alpha_abd((w1 >> 21) & 0x07),
                        decode_alpha_abd((w1 >> 3) & 0x07),
                        decode_alpha_c((w1 >> 18) & 0x07),
                        decode_alpha_abd(w1 & 0x07),
                    ],
                },
            ],
        }
    }

    pub(crate) fn uses_texel1(self, cycle_type: CycleType) -> bool {
        let cycle_count = match cycle_type {
            CycleType::OneCycle => 1,
            CycleType::TwoCycle => 2,
            CycleType::Copy | CycleType::Fill => 0,
        };
        self.cycles.iter().take(cycle_count).any(|cycle| {
            cycle
                .rgb
                .iter()
                .any(|source| matches!(source, ColorSource::Texel1 | ColorSource::Texel1Alpha))
                || cycle
                    .alpha
                    .iter()
                    .any(|source| matches!(source, AlphaSource::Texel1))
        })
    }
}

impl Default for CombinerMode {
    fn default() -> Self {
        // Neutral legacy/default path: TEXEL0 * SHADE for RGB and alpha.
        // A missing texture is supplied as white by the software evaluator,
        // preserving the original untextured shade-only fixture behavior.
        let modulate = CombinerCycle {
            rgb: [
                ColorSource::Texel0,
                ColorSource::Zero,
                ColorSource::Shade,
                ColorSource::Zero,
            ],
            alpha: [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Shade,
                AlphaSource::Zero,
            ],
        };
        CombinerMode {
            cycles: [modulate; 2],
        }
    }
}

impl OtherMode {
    pub fn raw_high(self) -> u32 {
        self.high
    }

    pub fn raw_low(self) -> u32 {
        self.low
    }

    pub fn cycle_type(self) -> CycleType {
        match (self.high >> 20) & 3 {
            0 => CycleType::OneCycle,
            1 => CycleType::TwoCycle,
            2 => CycleType::Copy,
            _ => CycleType::Fill,
        }
    }

    pub fn texture_filter(self) -> TextureFilter {
        match (self.high >> 12) & 3 {
            0 => TextureFilter::Point,
            1 => TextureFilter::Reserved,
            2 => TextureFilter::Bilinear,
            _ => TextureFilter::Average,
        }
    }

    pub fn rgb_dither(self) -> RgbDither {
        match (self.high >> 6) & 3 {
            0 => RgbDither::MagicSquare,
            1 => RgbDither::Bayer,
            2 => RgbDither::Noise,
            _ => RgbDither::Disabled,
        }
    }

    pub fn alpha_dither(self) -> AlphaDither {
        match (self.high >> 4) & 3 {
            0 => AlphaDither::Pattern,
            1 => AlphaDither::InversePattern,
            2 => AlphaDither::Noise,
            _ => AlphaDither::Disabled,
        }
    }

    pub fn combine_key(self) -> bool {
        self.high & (1 << 8) != 0
    }

    pub fn texture_convert(self) -> u8 {
        ((self.high >> 9) & 7) as u8
    }

    pub fn texture_lut(self) -> u8 {
        ((self.high >> 14) & 3) as u8
    }

    pub fn texture_lod(self) -> bool {
        self.high & (1 << 16) != 0
    }

    pub fn texture_detail(self) -> u8 {
        ((self.high >> 17) & 3) as u8
    }

    pub fn texture_perspective(self) -> bool {
        self.high & (1 << 19) != 0
    }

    pub fn one_primitive_pipeline(self) -> bool {
        self.high & (1 << 23) != 0
    }

    pub fn alpha_compare(self) -> AlphaCompare {
        match self.low & 3 {
            0 => AlphaCompare::None,
            1 => AlphaCompare::Threshold,
            2 => AlphaCompare::Reserved,
            _ => AlphaCompare::Dither,
        }
    }

    pub fn primitive_depth_source(self) -> bool {
        self.low & (1 << 2) != 0
    }

    pub fn antialias_enabled(self) -> bool {
        self.low & 0x0008 != 0
    }

    pub fn depth_compare_enabled(self) -> bool {
        self.low & 0x0010 != 0
    }

    pub fn depth_update_enabled(self) -> bool {
        self.low & 0x0020 != 0
    }

    pub fn image_read_enabled(self) -> bool {
        self.low & 0x0040 != 0
    }

    pub fn clear_on_coverage(self) -> bool {
        self.low & 0x0080 != 0
    }

    pub fn coverage_destination(self) -> CoverageDestination {
        match (self.low >> 8) & 3 {
            0 => CoverageDestination::Clamp,
            1 => CoverageDestination::Wrap,
            2 => CoverageDestination::Full,
            _ => CoverageDestination::Save,
        }
    }

    pub fn depth_mode(self) -> DepthMode {
        match (self.low >> 10) & 3 {
            0 => DepthMode::Opaque,
            1 => DepthMode::Interpenetrating,
            2 => DepthMode::Translucent,
            _ => DepthMode::Decal,
        }
    }

    pub fn coverage_times_alpha(self) -> bool {
        self.low & 0x1000 != 0
    }

    pub fn alpha_coverage_select(self) -> bool {
        self.low & 0x2000 != 0
    }

    pub fn force_blend(self) -> bool {
        self.low & 0x4000 != 0
    }

    pub fn blender_cycle_1(self) -> BlenderCycle {
        BlenderCycle {
            color_a: ((self.low >> 30) & 3) as u8,
            alpha_a: ((self.low >> 26) & 3) as u8,
            color_b: ((self.low >> 22) & 3) as u8,
            alpha_b: ((self.low >> 18) & 3) as u8,
        }
    }

    pub fn blender_cycle_2(self) -> BlenderCycle {
        BlenderCycle {
            color_a: ((self.low >> 28) & 3) as u8,
            alpha_a: ((self.low >> 24) & 3) as u8,
            color_b: ((self.low >> 20) & 3) as u8,
            alpha_b: ((self.low >> 16) & 3) as u8,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_raw(high: u32, low: u32, blend_color_alpha: u8) -> Self {
        Self {
            high,
            low,
            blend_color_alpha,
        }
    }
}

/// RDP color state snapshotted onto each emitted triangle.
///
/// This stays separate from the render/other-mode state being added on the
/// neighboring job: it contains `G_SETCOMBINE`, primitive/environment RGBA,
/// primitive LOD fraction, conversion constants, and chroma-key registers.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CombinerState {
    pub mode: CombinerMode,
    pub primitive: [u8; 4],
    pub environment: [u8; 4],
    pub min_lod_level: u8,
    pub prim_lod_fraction: u8,
    pub convert: ConvertState,
    pub key: KeyState,
}

/// Persistent chroma-key center, scale, and 4.8-width registers.
///
/// Public SGI *RDP Command Summary* Tables 29-30 define the split `SETKEYR`
/// and `SETKEYGB` wire layouts and the alpha-fixup equation. Keeping width in
/// its twelve-bit wire form preserves the documented `> 1.0` channel-disable
/// rule without round-tripping through floating point during decode.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyState {
    pub center: [u8; 3],
    pub scale: [u8; 3],
    pub width: [u16; 3],
}

impl KeyState {
    fn set_r(&mut self, w1: u32) {
        self.width[0] = ((w1 >> 16) & 0x0fff) as u16;
        self.center[0] = (w1 >> 8) as u8;
        self.scale[0] = w1 as u8;
    }

    fn set_gb(&mut self, w0: u32, w1: u32) {
        self.width[1] = ((w0 >> 12) & 0x0fff) as u16;
        self.width[2] = (w0 & 0x0fff) as u16;
        self.center[1] = (w1 >> 24) as u8;
        self.scale[1] = (w1 >> 16) as u8;
        self.center[2] = (w1 >> 8) as u8;
        self.scale[2] = w1 as u8;
    }

    pub(crate) fn center_unit(self) -> [f32; 3] {
        self.center.map(|value| f32::from(value) / 255.0)
    }

    pub(crate) fn scale_unit(self) -> [f32; 3] {
        self.scale.map(|value| f32::from(value) / 255.0)
    }

    pub(crate) fn alpha_from_key_prime(self, key_prime: [f32; 3]) -> f32 {
        let mut alpha = 1.0f32;
        for (channel, value) in key_prime.into_iter().enumerate() {
            let component = if self.width[channel] > 0x100 {
                // The public programming manual specifies width > 1.0 as
                // disabling keying for that channel.
                1.0
            } else {
                (f32::from(self.width[channel]) / 256.0 - value.abs()).clamp(0.0, 1.0)
            };
            alpha = alpha.min(component);
        }
        alpha
    }
}

/// Persistent `G_SETCONVERT` K0..K5 registers. SGI's public *RDP Command
/// Summary*, Table 28, defines six signed nine-bit fields and the two-stage
/// YUV conversion equations. Keeping the wire integers avoids losing their
/// distinct fixed-point interpretations: K0..K3 are S1.7 texture-filter
/// multipliers, K4 is an 8-bit combiner offset, and K5 is the combiner scale.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ConvertState {
    pub coefficients: [i16; 6],
}

impl Default for ConvertState {
    fn default() -> Self {
        // Public gbi.h G_CV_K0..G_CV_K5 defaults for YUV-to-RGB.
        Self {
            coefficients: [175, -43, -89, 222, 114, 42],
        }
    }
}

impl ConvertState {
    fn decode(w0: u32, w1: u32) -> Self {
        let signed_9 = |value: u32| ((value << 23) as i32 >> 23) as i16;
        Self {
            coefficients: [
                signed_9((w0 >> 13) & 0x1ff),
                signed_9((w0 >> 4) & 0x1ff),
                signed_9(((w0 & 0x0f) << 5) | ((w1 >> 27) & 0x1f)),
                signed_9((w1 >> 18) & 0x1ff),
                signed_9((w1 >> 9) & 0x1ff),
                signed_9(w1 & 0x1ff),
            ],
        }
    }

    pub(crate) fn convert_texel(self, texel: [u8; 4]) -> [u8; 4] {
        let [k0, k1, k2, k3, _, _] = self.coefficients;
        let y = i32::from(texel[0]);
        let u = i32::from(texel[1]) - 128;
        let v = i32::from(texel[2]) - 128;
        let multiply =
            |coefficient: i16, component: i32| (i32::from(coefficient) * component).div_euclid(128);
        let clamp = |value: i32| value.clamp(0, 255) as u8;
        [
            clamp(y + multiply(k0, v)),
            clamp(y + multiply(k1, u) + multiply(k2, v)),
            clamp(y + multiply(k3, u)),
            texel[3],
        ]
    }

    pub(crate) fn k4(self) -> f32 {
        f32::from(self.coefficients[4]) / 255.0
    }

    pub(crate) fn k5(self) -> f32 {
        f32::from(self.coefficients[5]) / 256.0
    }
}

fn decode_color_common(value: u32) -> ColorSource {
    match value {
        0 => ColorSource::Combined,
        1 => ColorSource::Texel0,
        2 => ColorSource::Texel1,
        3 => ColorSource::Primitive,
        4 => ColorSource::Shade,
        5 => ColorSource::Environment,
        _ => ColorSource::Zero,
    }
}

fn decode_color_a(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::One,
        7 => ColorSource::Noise,
        _ => ColorSource::Zero,
    }
}

fn decode_color_b(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::KeyCenter,
        7 => ColorSource::K4,
        _ => ColorSource::Zero,
    }
}

fn decode_color_c(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::KeyScale,
        7 => ColorSource::CombinedAlpha,
        8 => ColorSource::Texel0Alpha,
        9 => ColorSource::Texel1Alpha,
        10 => ColorSource::PrimitiveAlpha,
        11 => ColorSource::ShadeAlpha,
        12 => ColorSource::EnvironmentAlpha,
        13 => ColorSource::LodFraction,
        14 => ColorSource::PrimLodFraction,
        15 => ColorSource::K5,
        _ => ColorSource::Zero,
    }
}

fn decode_color_d(value: u32) -> ColorSource {
    match value {
        0..=5 => decode_color_common(value),
        6 => ColorSource::One,
        _ => ColorSource::Zero,
    }
}

fn decode_alpha_abd(value: u32) -> AlphaSource {
    match value {
        0 => AlphaSource::Combined,
        1 => AlphaSource::Texel0,
        2 => AlphaSource::Texel1,
        3 => AlphaSource::Primitive,
        4 => AlphaSource::Shade,
        5 => AlphaSource::Environment,
        6 => AlphaSource::One,
        _ => AlphaSource::Zero,
    }
}

fn decode_alpha_c(value: u32) -> AlphaSource {
    match value {
        0 => AlphaSource::LodFraction,
        1 => AlphaSource::Texel0,
        2 => AlphaSource::Texel1,
        3 => AlphaSource::Primitive,
        4 => AlphaSource::Shade,
        5 => AlphaSource::Environment,
        6 => AlphaSource::PrimLodFraction,
        _ => AlphaSource::Zero,
    }
}

/// An immutable texture view plus its complete public per-axis tile-coordinate
/// mode, ready for the rasterizer to sample. Display-list textures retain a
/// physical TMEM snapshot and render-tile descriptor; hand-built fixtures use
/// the RGBA8888 row-major buffer. Both backings are reference-counted so many
/// primitives can share one command-ordered image (`F3DEX2-CONCEPTS.md` §5.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    /// Public GBI image format/size wire values retained for copy-mode
    /// legality checks and future format-preserving framebuffer copies.
    pub format: u8,
    pub size: u8,
    pub width: u32,
    pub height: u32,
    /// RGBA8888, `width * height * 4` bytes, row-major top-left origin.
    pub texels: std::rc::Rc<Vec<u8>>,
    /// S-axis clamp-enable bit. A zero mask still implies clamp regardless of
    /// this bit, per Programming Manual Chapter 13, "Clamp S,T".
    pub clamp_s: bool,
    /// T-axis clamp-enable bit.
    pub clamp_t: bool,
    /// Per-axis mirror-enable bits.
    pub mirror_s: bool,
    pub mirror_t: bool,
    /// Number of low coordinate bits passed by wrapping (0..=15). Zero means
    /// no mask and therefore implicit clamp.
    pub mask_s: u8,
    pub mask_t: u8,
    /// Public four-bit post-perspective coordinate shift encodings.
    pub shift_s: u8,
    pub shift_t: u8,
    /// Tile-coordinate origin in texels (`uls/ult` quarter-texel fields).
    /// Vertex S/T are expressed in the image's coordinate domain, so the
    /// sampled coordinate is relative to this loaded tile origin.
    pub origin_s: f32,
    pub origin_t: f32,
    /// Immutable physical TMEM snapshot plus the render-tile descriptor.
    /// Display-list textures use this backing so sampling observes tile base,
    /// line stride, odd-row bank swapping, format reinterpretation, and data
    /// loaded through a different tile descriptor. Hand-built reference
    /// fixtures retain the decoded `texels` backing above.
    pub(crate) tmem: Option<std::rc::Rc<TmemTexture>>,
    /// Immutable tile set captured when a textured primitive is emitted.
    /// Loaded textures inside the snapshot never carry another snapshot, so
    /// the `Rc` indirection is finite and many primitives can share it.
    pub(crate) lod: Option<std::rc::Rc<TextureLodSnapshot>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextureLodSnapshot {
    tiles: [Option<Texture>; 8],
    primitive_tile: u8,
    max_level: u8,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct TextureDerivatives {
    pub dsdx: f32,
    pub dtdx: f32,
    pub dsdy: f32,
    pub dtdy: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct TextureSampleRequest {
    pub s: f32,
    pub t: f32,
    pub derivatives: TextureDerivatives,
    pub other_mode: OtherMode,
    pub convert: ConvertState,
    pub min_level: u8,
    pub require_texel1: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct TextureLodSelection {
    pub tile0: u8,
    pub tile1: u8,
    pub fraction: f32,
}

/// Post-perspective texture coordinate in the public signed S10.5 range.
///
/// Programming Manual 13.7 states that the texture unit has five fractional
/// coordinate bits, and 13.11 limits valid input to -1024..+1023.99. Tile
/// shifts operate after that boundary and can widen the integer magnitude, so
/// they return a distinct host accumulator instead of weakening this type's
/// public range. Interpolation reaches this boundary as host float today, so
/// conversion deliberately floors to the containing 1/32-texel cell. That
/// conversion is a bounded host policy until silicon reciprocal/quantization
/// traces exist.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct TextureCoordinateS10_5(i16);

impl TextureCoordinateS10_5 {
    const FRACTION_BITS: u32 = 5;
    const SCALE: i64 = 1 << Self::FRACTION_BITS;

    fn from_texels_bounded(coordinate: f32) -> Self {
        if !coordinate.is_finite() {
            crate::render_unsupported_panic(
                "render.gbi.texture-coordinate-range",
                "non-finite coordinate reached RDP texture sampler",
            );
        }
        let scaled = f64::from(coordinate) * Self::SCALE as f64;
        let quantized = scaled.floor();
        if !(quantized >= f64::from(i16::MIN) && quantized <= f64::from(i16::MAX)) {
            crate::render_unsupported_panic(
                "render.gbi.texture-coordinate-range",
                "RDP texture coordinate lies outside the public signed S10.5 range",
            );
        }
        Self(quantized as i16)
    }

    fn shifted(self, encoded: u8) -> TextureCoordinateAccumulator5 {
        let raw = i64::from(self.0);
        match encoded {
            0 => TextureCoordinateAccumulator5(raw),
            1..=10 => TextureCoordinateAccumulator5(raw >> encoded),
            11..=15 => TextureCoordinateAccumulator5(
                raw.checked_mul(1_i64 << (16 - encoded))
                    .expect("RDP texture coordinate left shift overflowed fixed-point host range"),
            ),
            _ => unreachable!("G_SETTILE shift is a four-bit field"),
        }
    }
}

/// Wider five-fractional-bit host accumulator created only after a public tile
/// shift. Left shifts 11..=15 can expand a valid signed S10.5 input through an
/// S15.5-equivalent magnitude before the S10.2 tile origin is subtracted.
/// This width is a safe host mechanism, not a claim about a silicon register.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct TextureCoordinateAccumulator5(i64);

impl TextureCoordinateAccumulator5 {
    fn relative_to(self, origin: TextureCoordinateS10_5) -> Self {
        Self(
            self.0
                .checked_sub(i64::from(origin.0))
                .expect("RDP texture origin subtraction overflowed fixed-point host range"),
        )
    }

    fn texel(self) -> i64 {
        self.0.div_euclid(TextureCoordinateS10_5::SCALE)
    }

    fn fraction(self) -> i64 {
        self.0.rem_euclid(TextureCoordinateS10_5::SCALE)
    }
}

fn filter_three_nearest_s10_5(samples: [[u8; 4]; 4], sf: i64, tf: i64) -> [u8; 4] {
    debug_assert!((0..TextureCoordinateS10_5::SCALE).contains(&sf));
    debug_assert!((0..TextureCoordinateS10_5::SCALE).contains(&tf));
    std::array::from_fn(|channel| {
        let [c00, c10, c01, c11] = samples.map(|sample| i64::from(sample[channel]));
        let value = if sf + tf <= TextureCoordinateS10_5::SCALE {
            c00 * TextureCoordinateS10_5::SCALE + sf * (c10 - c00) + tf * (c01 - c00)
        } else {
            c11 * TextureCoordinateS10_5::SCALE
                + (TextureCoordinateS10_5::SCALE - sf) * (c01 - c11)
                + (TextureCoordinateS10_5::SCALE - tf) * (c10 - c11)
        };
        // Preserve the reference lane's round-to-nearest output policy;
        // public documentation does not establish the silicon filter
        // accumulator width or tie rule.
        ((value + TextureCoordinateS10_5::SCALE / 2) / TextureCoordinateS10_5::SCALE).clamp(0, 255)
            as u8
    })
}

/// Which public texture-coordinate path owns clamp selection. Ordinary
/// point/filter sampling consumes the programmed per-axis clamp bit (with the
/// zero-mask implicit clamp rule). Programming Manual Chapter 13.11 states
/// that copy mode disables clamping, so that path cannot observe those bits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TextureAddressMode {
    Programmed,
    Copy,
}

fn texture_axis_address(
    coordinate: i64,
    dimension: u32,
    clamp: bool,
    mirror: bool,
    mask: u8,
    mode: TextureAddressMode,
) -> u32 {
    if dimension == 0 {
        return 0;
    }
    assert!(mask <= 15, "G_SETTILE mask exceeds its four-bit field");

    // Outside copy mode, mask zero forces clamping. With a nonzero mask, the
    // explicit clamp bit clamps before mirror/mask; this reproduces the
    // manual's SH=11/mask=2 example, where every input above 11 resolves to
    // mirrored texel 3. Copy mode bypasses both clamp sources and proceeds to
    // its documented wrap/mirror addressing.
    let clamps = mode == TextureAddressMode::Programmed && (mask == 0 || clamp);
    let coordinate = if clamps {
        coordinate.clamp(0, i64::from(dimension) - 1)
    } else {
        coordinate
    };
    if mask == 0 {
        return coordinate as u32;
    }

    let low_mask = (1_i64 << mask) - 1;
    if mirror && coordinate & (1_i64 << mask) != 0 {
        ((!coordinate) & low_mask) as u32
    } else {
        (coordinate & low_mask) as u32
    }
}

/// Per-primitive snapshot of the RDP scissor rectangle, in screen pixels.
/// Lower-right edges are exclusive. `field`/`keep_odd` retain the public
/// Set Scissor command's interlace controls: when enabled, an entire opposite-
/// parity scanline is rejected before coverage or image writes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScissorRect {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
    pub field: bool,
    pub keep_odd: bool,
}

impl ScissorRect {
    pub(crate) fn framebuffer(width: u32, height: u32) -> Self {
        Self {
            ulx: 0.0,
            uly: 0.0,
            lrx: width as f32,
            lry: height as f32,
            field: false,
            keep_odd: false,
        }
    }

    pub(crate) fn line_enabled(self, y: i32) -> bool {
        !self.field || y.rem_euclid(2) == i32::from(self.keep_odd)
    }
}

impl Texture {
    pub(crate) fn with_lod_snapshot(
        mut self,
        tiles: [Option<Texture>; 8],
        primitive_tile: u8,
        max_level: u8,
    ) -> Self {
        debug_assert!(tiles.iter().flatten().all(|texture| texture.lod.is_none()));
        self.lod = Some(std::rc::Rc::new(TextureLodSnapshot {
            tiles,
            primitive_tile,
            max_level,
        }));
        self
    }

    /// Point-sample at texel coordinates `(s, t)`, applying the tile's shift,
    /// clamp, mirror, and mask state per axis. Integer coordinates address
    /// texel centers in the public GBI coordinate domain; fractional parts
    /// select the same texel until the next integer boundary.
    pub fn sample(&self, s: f32, t: f32) -> [u8; 4] {
        self.sample_filtered(s, t, TextureFilter::Point)
    }

    /// Point-sample through the copy-cycle coordinate path. Chapter 13.11,
    /// "Restrictions," specifies that copy implicitly disables clamping while
    /// retaining wrap/mirror for supported texel sizes.
    pub(crate) fn sample_copy(&self, s: f32, t: f32) -> [u8; 4] {
        self.sample_filtered_with_address_mode(s, t, TextureFilter::Point, TextureAddressMode::Copy)
    }

    /// Sample through the RDP texture-filter mode.
    ///
    /// Nintendo's Programming Manual, "TF: Texture Filter" and "Sampling
    /// Overview", defines point selection, a four-texel box average, and the
    /// hardware's bilerp optimization: triangular interpolation of the three
    /// nearest samples selected by the sample's position in the 2x2 cell.
    /// Keeping this on `Texture` makes triangles and rectangles consume the
    /// same clean-room filter rather than growing backend-specific samplers.
    pub fn sample_filtered(&self, s: f32, t: f32, filter: TextureFilter) -> [u8; 4] {
        self.sample_filtered_with_address_mode(s, t, filter, TextureAddressMode::Programmed)
    }

    fn sample_filtered_with_address_mode(
        &self,
        s: f32,
        t: f32,
        filter: TextureFilter,
        address_mode: TextureAddressMode,
    ) -> [u8; 4] {
        let texel = |x: i64, y: i64| -> [u8; 4] {
            let x = texture_axis_address(
                x,
                self.width,
                self.clamp_s,
                self.mirror_s,
                self.mask_s,
                address_mode,
            );
            let y = texture_axis_address(
                y,
                self.height,
                self.clamp_t,
                self.mirror_t,
                self.mask_t,
                address_mode,
            );
            if let Some(tmem) = &self.tmem {
                return tmem.sample(x as usize, y as usize);
            }
            assert!(
                x < self.width && y < self.height,
                "G_SETTILE masks ({}, {}) address texel ({x}, {y}) outside decoded {}x{} fixture",
                self.mask_s,
                self.mask_t,
                self.width,
                self.height,
            );
            let offset = ((y * self.width + x) * 4) as usize;
            assert!(
                offset + 4 <= self.texels.len(),
                "texture sample ({x}, {y}) exceeds {}x{} RGBA buffer of {} bytes",
                self.width,
                self.height,
                self.texels.len()
            );
            [
                self.texels[offset],
                self.texels[offset + 1],
                self.texels[offset + 2],
                self.texels[offset + 3],
            ]
        };

        let s = TextureCoordinateS10_5::from_texels_bounded(s)
            .shifted(self.shift_s)
            .relative_to(TextureCoordinateS10_5::from_texels_bounded(self.origin_s));
        let t = TextureCoordinateS10_5::from_texels_bounded(t)
            .shifted(self.shift_t)
            .relative_to(TextureCoordinateS10_5::from_texels_bounded(self.origin_t));
        let s0 = s.texel();
        let t0 = t.texel();
        if filter == TextureFilter::Point {
            return texel(s0, t0);
        }
        assert_ne!(
            filter,
            TextureFilter::Reserved,
            "reserved RDP texture-filter mode reached sampler"
        );

        let samples = [
            texel(s0, t0),
            texel(s0 + 1, t0),
            texel(s0, t0 + 1),
            texel(s0 + 1, t0 + 1),
        ];
        if filter == TextureFilter::Average {
            return std::array::from_fn(|channel| {
                let sum = samples
                    .iter()
                    .map(|sample| u16::from(sample[channel]))
                    .sum::<u16>();
                ((sum + 2) / 4) as u8
            });
        }

        let sf = s.fraction();
        let tf = t.fraction();
        filter_three_nearest_s10_5(samples, sf, tf)
    }

    /// Run the public texture-filter conversion selection. `G_TC_CONV`
    /// performs point conversion, `G_TC_FILTCONV` filters then converts, and
    /// `G_TC_FILT` returns the filtered texel unchanged.
    pub(crate) fn sample_rdp(
        &self,
        s: f32,
        t: f32,
        other_mode: OtherMode,
        convert: ConvertState,
    ) -> [u8; 4] {
        match other_mode.texture_convert() {
            0 => convert.convert_texel(self.sample_filtered(s, t, TextureFilter::Point)),
            5 => convert.convert_texel(self.sample_filtered(s, t, other_mode.texture_filter())),
            6 => self.sample_filtered(s, t, other_mode.texture_filter()),
            mode => panic!("reserved RDP texture-convert mode {mode} reached sampler"),
        }
    }

    fn lod_selection(
        snapshot: &TextureLodSnapshot,
        derivatives: TextureDerivatives,
        other_mode: OtherMode,
        min_level: u8,
    ) -> TextureLodSelection {
        if !other_mode.texture_lod() {
            return TextureLodSelection {
                tile0: snapshot.primitive_tile,
                tile1: snapshot.primitive_tile.wrapping_add(1) & 7,
                fraction: 0.0,
            };
        }

        let detail = other_mode.texture_detail();
        assert_ne!(
            detail, 3,
            "reserved RDP texture-detail mode reached sampler"
        );
        let lod = derivatives
            .dsdx
            .abs()
            .max(derivatives.dtdx.abs())
            .max(derivatives.dsdy.abs())
            .max(derivatives.dtdy.abs());
        assert!(
            lod.is_finite(),
            "non-finite texture derivative reached RDP LOD"
        );
        let minimum = f32::from(min_level) / 255.0;
        let clamped = lod.max(minimum);
        let magnifying = clamped <= 1.0;
        let unclamped_tile = if clamped < 2.0 {
            0
        } else {
            clamped.floor().log2().floor() as u8
        };
        let level = unclamped_tile.min(snapshot.max_level.min(7));
        let base_fraction = if magnifying {
            clamped
        } else {
            (clamped / (1_u32 << level) as f32 - 1.0).clamp(0.0, 1.0)
        };

        // Programming Manual Chapter 13.7 Tables 3-4. Detail shifts both
        // cycle tiles above the primitive detail tile outside magnification;
        // sharpen keeps the ordinary adjacent pair but extrapolates with a
        // negative fraction while magnifying. Clamp mode reuses the finest
        // tile for both cycles during magnification.
        let (offset0, offset1, fraction) = match (detail, magnifying) {
            (2, true) => (0, 1, base_fraction.max(minimum)),
            (2, false) => (
                level.saturating_add(1),
                level.saturating_add(2),
                base_fraction,
            ),
            (1, true) => (0, 1, clamped - 1.0),
            (1, false) => (level, level.saturating_add(1), base_fraction),
            (0, true) => (0, 0, base_fraction),
            (0, false) => (
                level,
                level.saturating_add(1).min(snapshot.max_level),
                base_fraction,
            ),
            _ => unreachable!("texture detail field is two bits"),
        };
        TextureLodSelection {
            tile0: snapshot.primitive_tile.wrapping_add(offset0) & 7,
            tile1: snapshot.primitive_tile.wrapping_add(offset1) & 7,
            fraction,
        }
    }

    pub(crate) fn sample_rdp_pair(
        &self,
        fallback_texel1: Option<&Texture>,
        request: TextureSampleRequest,
    ) -> ([u8; 4], [u8; 4], f32) {
        let TextureSampleRequest {
            s,
            t,
            derivatives,
            other_mode,
            convert,
            min_level,
            require_texel1,
        } = request;
        let Some(snapshot) = self.lod.as_deref() else {
            assert!(
                !other_mode.texture_lod() && other_mode.texture_detail() == 0,
                "texture LOD/detail reached sampler without an immutable tile snapshot"
            );
            return (
                self.sample_rdp(s, t, other_mode, convert),
                fallback_texel1
                    .or_else(|| (!require_texel1).then_some(self))
                    .expect("RDP combiner selected TEXEL1 without a decoded tile+1 image")
                    .sample_rdp(s, t, other_mode, convert),
                0.0,
            );
        };
        let selection = Self::lod_selection(snapshot, derivatives, other_mode, min_level);
        let tile0 = snapshot.tiles[usize::from(selection.tile0)]
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "RDP LOD selected tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
                    selection.tile0
                )
            });
        let tile1 = snapshot.tiles[usize::from(selection.tile1)]
            .as_ref()
            .or_else(|| (!other_mode.texture_lod() && !require_texel1).then_some(tile0))
            .unwrap_or_else(|| {
                panic!(
                    "RDP LOD selected tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
                    selection.tile1
                )
            });
        (
            tile0.sample_rdp(s, t, other_mode, convert),
            tile1.sample_rdp(s, t, other_mode, convert),
            selection.fraction,
        )
    }
}

/// A decoded, screen-space-ready triangle (three already-resolved
/// vertices) -- the display-list decoder's actual output, consumed by the
/// rasterizer in `raster.rs`.
#[derive(Clone, Debug, Default)]
pub struct Triangle {
    pub v: [Vertex; 3],
    /// RDP scissor active when this triangle was emitted. `None` preserves
    /// framebuffer-only clipping for the legacy fixture decoder.
    pub scissor: Option<ScissorRect>,
    /// The culling mode in effect (from `G_GEOMETRYMODE`) when this triangle
    /// was emitted. Carried per-triangle because geometry mode is decode-time
    /// RSP state that can change between `G_TRI*` commands; the rasterizer
    /// reads it to cull by winding. `None` for the simple reference path.
    pub cull: CullMode,
    /// The texture bound (via `G_TEXTURE` enable + a loaded tile) when this
    /// triangle was emitted, if any. `None` means texturing was disabled (or
    /// this is a fixture-only primitive); an enabled tile without live TMEM
    /// traps during decode rather than arriving here as white. The rasterizer
    /// modulates the sampled texel by the interpolated shade color
    /// (`F3DEX2-CONCEPTS.md` §5.2, the MODULATE combiner).
    pub texture: Option<Texture>,
    /// RDP other-mode and alpha-threshold state in effect when this triangle
    /// was emitted. Like culling/texture, this is snapshotted per triangle
    /// because later display-list commands may mutate global decode state.
    pub other_mode: OtherMode,
    /// Color-combiner mode and its primitive/environment inputs in effect
    /// when this triangle was emitted. Kept per-triangle for the same reason
    /// as texture/cull state: later display-list commands may change it.
    pub combiner: CombinerState,
    /// RDP framebuffer blender state in effect when this triangle was emitted.
    /// This is derived from the same other-mode snapshot: cycle type,
    /// `FORCE_BL`, the two `GBL_c1`/`GBL_c2` selector tuples, and the constant
    /// colors those tuples can address.
    pub blender: BlenderState,
}

/// One F3DEX2/L3DEX line after vertex-cache resolution.
///
/// Public `gSPLineW3D` defines `width = 1.5 + wd / 2` pixels. The command's
/// flat-shading flag is represented by endpoint order on the wire, so `v[0]`
/// is also the selected flat color when smooth shading is disabled.
#[derive(Clone, Debug)]
pub struct Line {
    pub v: [Vertex; 2],
    pub width: f32,
    pub smooth_shading: bool,
    pub scissor: Option<ScissorRect>,
    pub texture: Option<Texture>,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
}

struct LineDecodeSnapshot {
    smooth_shading: bool,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
    scissor: Option<ScissorRect>,
    viewport: Option<Viewport>,
    clip_ratio: ClipRatio,
}

/// The four 64-bit edge-coefficient words shared by every raw RDP triangle.
/// Y values are signed S11.2 and X/slopes are signed 16.16 on the wire.
///
/// Provenance: SGI *RDP Command Summary*, Tables 11-12 (1996-04-11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpEdgeCoefficients {
    pub right_major: bool,
    pub level: u8,
    pub tile: u8,
    pub yl: i16,
    pub ym: i16,
    pub yh: i16,
    pub xl: i32,
    pub dxldy: i32,
    pub xh: i32,
    pub dxhdy: i32,
    pub xm: i32,
    pub dxmdy: i32,
}

/// The two 64-bit Z coefficient words appended by raw opcodes with bit 0 set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpZCoefficients {
    pub z: i32,
    pub dzdx: i32,
    pub dzde: i32,
    pub dzdy: i32,
}

/// The eight 64-bit shade coefficient words appended by raw opcodes with
/// bit 2 set. Each component is retained as signed 16.16 so negative color
/// gradients survive ingestion.
///
/// Provenance: SGI *RDP Command Summary*, Table 13 (1996-04-11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpShadeCoefficients {
    pub color: [i32; 4],
    pub dcdx: [i32; 4],
    pub dcde: [i32; 4],
    pub dcdy: [i32; 4],
}

/// The eight 64-bit texture coefficient words appended by raw opcodes with
/// bit 1 set. S, T, normalized inverse-W, and their gradients remain signed
/// 16.16 values until vertex reconstruction.
///
/// Provenance: SGI *RDP Command Summary*, Table 14 (1996-04-11).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RdpTextureCoefficients {
    pub stw: [i32; 3],
    pub dstdx: [i32; 3],
    pub dstde: [i32; 3],
    pub dstdy: [i32; 3],
}

/// One raw RDP triangle retaining the hardware's edge and attribute planes.
/// Keeping this distinct from [`Triangle`] prevents the command decoder from
/// throwing away major-edge direction and `d/de` stepping before rasterization.
#[derive(Clone, Debug)]
pub struct RawRdpTriangle {
    pub edge: RdpEdgeCoefficients,
    pub shade: Option<RdpShadeCoefficients>,
    pub texture_coefficients: Option<RdpTextureCoefficients>,
    pub z: Option<RdpZCoefficients>,
    pub texture: Option<Texture>,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
    pub scissor: Option<ScissorRect>,
}

/// One RDP color-image descriptor from `G_SETCIMG`.
///
/// Format and size retain their public GBI wire values. The reference path
/// supports all three public RDP memory-interface color-image sizes: 8-bit
/// intensity, RGBA16, and RGBA32. Retaining the raw format field lets invalid
/// 16/32-bit combinations fail by name while the size-defined 8-bit layout is
/// represented without inventing a palette.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColorImage {
    pub format: u8,
    pub size: u8,
    pub width: u16,
    pub address: u32,
}

/// Legal public RDP color-image memory layouts.
///
/// The memory interface exposes one size-defined 8-bit layout plus RGBA16 and
/// RGBA32. Classifying the raw `G_SETCIMG` fields once prevents individual
/// import, raster, and writeback paths from accepting different format sets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorImageLayout {
    Index8,
    Rgba16,
    Rgba32,
}

impl ColorImageLayout {
    pub const ALL: [Self; 3] = [Self::Index8, Self::Rgba16, Self::Rgba32];

    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Index8 => 1,
            Self::Rgba16 => 2,
            Self::Rgba32 => 4,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Index8 => "I8/CI8",
            Self::Rgba16 => "RGBA16",
            Self::Rgba32 => "RGBA32",
        }
    }
}

/// One fully classified color-image target switch.
///
/// The RDP permits every transition among its three public memory layouts.
/// Constructing this value from the raw `G_SETCIMG` descriptors makes the
/// admission check a single, typed boundary shared by commit and import;
/// unsupported format/size pairs trap before either side mutates RDRAM.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColorImageLayoutTransition {
    pub from: ColorImageLayout,
    pub to: ColorImageLayout,
}

/// RDP depth-image register. The public command carries only a DRAM address;
/// dimensions follow the active color image/scissor state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DepthImage {
    pub address: u32,
}

/// Uniform RDP primitive Z/DeltaZ registers written by `G_SETPRIMDEPTH`.
/// Public libultra packs Z in `w1[31:16]` and DeltaZ in `w1[15:0]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PrimitiveDepth {
    pub z: u16,
    pub delta_z: u16,
}

impl ColorImage {
    pub const RGBA_FORMAT: u8 = 0;
    pub const CI_FORMAT: u8 = 2;
    pub const BITS_8: u8 = 1;
    pub const BITS_16: u8 = 2;
    pub const BITS_32: u8 = 3;

    pub const fn layout(self) -> Option<ColorImageLayout> {
        if self.size == Self::BITS_8 {
            Some(ColorImageLayout::Index8)
        } else if self.format == Self::RGBA_FORMAT && self.size == Self::BITS_16 {
            Some(ColorImageLayout::Rgba16)
        } else if self.format == Self::RGBA_FORMAT && self.size == Self::BITS_32 {
            Some(ColorImageLayout::Rgba32)
        } else {
            None
        }
    }

    pub const fn is_rgba16(self) -> bool {
        matches!(self.layout(), Some(ColorImageLayout::Rgba16))
    }

    pub const fn is_rgba32(self) -> bool {
        matches!(self.layout(), Some(ColorImageLayout::Rgba32))
    }

    pub const fn is_intensity8(self) -> bool {
        matches!(self.layout(), Some(ColorImageLayout::Index8))
    }

    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self.layout() {
            Some(layout) => Some(layout.bytes_per_pixel()),
            None => None,
        }
    }

    pub fn transition_to(self, next: Self) -> ColorImageLayoutTransition {
        let from = self.layout().unwrap_or_else(|| {
            crate::render_unsupported_panic(
                "render.gbi.color-image-layout",
                format!(
                    "unsupported source color-image layout: format={} size={}",
                    self.format, self.size
                ),
            )
        });
        let to = next.layout().unwrap_or_else(|| {
            crate::render_unsupported_panic(
                "render.gbi.color-image-layout",
                format!(
                    "unsupported destination color-image layout: format={} size={}",
                    next.format, next.size
                ),
            )
        });
        ColorImageLayoutTransition { from, to }
    }
}

/// One rectangle primitive with all RDP state required at its command position.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FillRectangle {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
    pub fill_color: u32,
    pub cycle_type: CycleType,
    pub scissor: Option<ScissorRect>,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
}

/// One complete 16-byte texture-rectangle command and its decode-time RDP
/// state. Screen coordinates are pixels, texture origins are texels, and the
/// gradients remain raw signed S5.10 values so cycle-specific execution can
/// apply the documented stepping rule without losing precision.
#[derive(Clone, Debug)]
pub struct TextureRectangle {
    pub ulx: f32,
    pub uly: f32,
    pub lrx: f32,
    pub lry: f32,
    pub tile: u8,
    pub s: f32,
    pub t: f32,
    pub dsdx: i16,
    pub dtdy: i16,
    pub flip: bool,
    pub other_mode: OtherMode,
    pub combiner: CombinerState,
    pub blender: BlenderState,
    pub scissor: Option<ScissorRect>,
    pub texture: Option<Texture>,
    /// With LOD disabled, TEXEL1 comes from the tile immediately after the
    /// command's TEXEL0 tile (N64 Graphics Tutorial, "Multi-tile Texture
    /// Rectangles"). Retaining it makes two-cycle rectangle programs real
    /// rather than aliasing TEXEL1 to TEXEL0.
    pub texture1: Option<Texture>,
}

/// Ordered RSP/RDP work produced by the F3DEX2 decoder.
///
/// A triangle-only return type loses framebuffer changes, fills, and sync
/// boundaries. This stream is the shared representation that later texrect,
/// copy-cycle, raw-RDP, and framebuffer-format work extends without another
/// decoder/backend seam change.
#[derive(Clone, Debug)]
pub enum RenderOp {
    Triangle(Triangle),
    Line(Line),
    RawTriangle(RawRdpTriangle),
    SetColorImage(ColorImage),
    SetDepthImage(DepthImage),
    SetPrimitiveDepth(PrimitiveDepth),
    FillRectangle(FillRectangle),
    TextureRectangle(TextureRectangle),
    FullSync,
}

/// One color input selected for the RDP blender's `P` or `M` term.
/// Values are the public `G_BL_CLR_*` encodings (gbi.h:612-615).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BlendColorInput {
    #[default]
    Combined,
    Framebuffer,
    Blend,
    Fog,
}

/// The multiplier selected for the blender's `A` term (`G_BL_A_*`,
/// gbi.h:618-622).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BlendAlphaInput {
    #[default]
    Combined,
    Fog,
    Shade,
    Zero,
}

/// The multiplier selected for the blender's `B` term (`G_BL_1MA`,
/// `G_BL_A_MEM`, `G_BL_1`, `G_BL_0`; gbi.h:616-622).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BlendBInput {
    #[default]
    OneMinusA,
    FramebufferAlpha,
    One,
    Zero,
}

/// One `GBL_c1`/`GBL_c2` tuple, evaluated as `P*A + M*B`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlendCycle {
    pub p: BlendColorInput,
    pub a: BlendAlphaInput,
    pub m: BlendColorInput,
    pub b: BlendBInput,
}

/// Minimal, per-triangle RDP blender snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlenderState {
    /// 0 for copy/fill (blender bypass), 1 for `G_CYC_1CYCLE`, 2 for
    /// `G_CYC_2CYCLE` (gbi.h:527-531).
    pub cycle_count: u8,
    pub force_blend: bool,
    pub cycles: [BlendCycle; 2],
    pub blend_color: [u8; 4],
    pub fog_color: [u8; 4],
}

impl BlenderState {
    fn from_other_mode(low: u32, high: u32, blend_color: [u8; 4], fog_color: [u8; 4]) -> Self {
        let color = |bits: u32| match bits & 3 {
            0 => BlendColorInput::Combined,
            1 => BlendColorInput::Framebuffer,
            2 => BlendColorInput::Blend,
            _ => BlendColorInput::Fog,
        };
        let alpha = |bits: u32| match bits & 3 {
            0 => BlendAlphaInput::Combined,
            1 => BlendAlphaInput::Fog,
            2 => BlendAlphaInput::Shade,
            _ => BlendAlphaInput::Zero,
        };
        let b = |bits: u32| match bits & 3 {
            0 => BlendBInput::OneMinusA,
            1 => BlendBInput::FramebufferAlpha,
            2 => BlendBInput::One,
            _ => BlendBInput::Zero,
        };
        let cycle_type = (high >> 20) & 3;
        BlenderState {
            cycle_count: match cycle_type {
                0 => 1,
                1 => 2,
                _ => 0,
            },
            force_blend: low & 0x0000_4000 != 0,
            // gbi.h:624-627: c1 fields at 30/26/22/18, c2 at
            // 28/24/20/16. Keeping that order visible makes merges with the
            // fuller othermode decoder mechanical.
            cycles: [
                BlendCycle {
                    p: color(low >> 30),
                    a: alpha(low >> 26),
                    m: color(low >> 22),
                    b: b(low >> 18),
                },
                BlendCycle {
                    p: color(low >> 28),
                    a: alpha(low >> 24),
                    m: color(low >> 20),
                    b: b(low >> 16),
                },
            ],
            blend_color,
            fog_color,
        }
    }
}

/// The N64 SDK's public per-vertex wire format (`Vtx_t`): 16 bytes --
/// `ob[3]` (s16 x/y/z), `flag` (u16, unused here), `tc[2]` (s16 st, unused
/// here), `cn[4]` (u8 r/g/b/a). x/y/z are read as model-space coords and
/// transformed through the active matrix stack; `cn` is a flat vertex color.
const VTX_STRIDE: usize = 16;

/// A 4x4 column-vector transform (row-major storage: `m[row][col]`), f32.
/// Built from an N64 fixed-point `Mtx` (see `read_mtx`) or the identity.
type Mat4 = [[f32; 4]; 4];

fn identity() -> Mat4 {
    let mut m = [[0.0f32; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// TEMP instrumentation (env `FN64_DUMP_PROJ=1`): true only while dumping the
/// projection/vertex data for the FIRST substantial gameplay frame, then it
/// self-disables so the log is one frame, not the whole boot. Gated entirely
/// behind the env var; no cost when unset. Remove/keep behind the flag.
#[cfg(not(test))]
mod projdump {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INIT: AtomicBool = AtomicBool::new(false);
    static VTX_LOGGED: AtomicU64 = AtomicU64::new(0);
    // clip-w histogram counters for the frame:
    pub static W_TOTAL: AtomicU64 = AtomicU64::new(0);
    pub static W_ONSCREEN: AtomicU64 = AtomicU64::new(0);
    pub static W_PATHOLOGICAL: AtomicU64 = AtomicU64::new(0);
    // screen-space depth (pz) range tracker (stored as i32 bits of f32):
    pub static PZ_MIN: AtomicU64 = AtomicU64::new(u64::MAX);
    pub static PZ_MAX: AtomicU64 = AtomicU64::new(0);

    /// Record one screen-space depth `pz` into the frame's [min,max] tracker.
    pub fn note_pz(pz: f32) {
        if !on() || !pz.is_finite() {
            return;
        }
        // Offset f32 into a monotonic u64 key so min/max compares work.
        let key = (pz * 1000.0) as i64 + (1i64 << 40);
        let key = key.max(0) as u64;
        PZ_MIN.fetch_min(key, Ordering::Relaxed);
        PZ_MAX.fetch_max(key, Ordering::Relaxed);
    }

    pub fn on() -> bool {
        if !INIT.swap(true, Ordering::Relaxed) {
            ENABLED.store(crate::debug_flag("FN64_DUMP_PROJ"), Ordering::Relaxed);
        }
        ENABLED.load(Ordering::Relaxed)
    }
    /// Only log the first N vertices verbosely, but keep counting all.
    pub fn should_log_vtx() -> bool {
        on() && VTX_LOGGED.fetch_add(1, Ordering::Relaxed) < 24
    }
    /// Reset per-frame counters so a summary reflects ONE frame, not the
    /// cumulative boot. Called at the start of each F3DEX2 decode.
    pub fn reset_frame() {
        if !on() {
            return;
        }
        W_TOTAL.store(0, Ordering::Relaxed);
        W_ONSCREEN.store(0, Ordering::Relaxed);
        W_PATHOLOGICAL.store(0, Ordering::Relaxed);
        PZ_MIN.store(u64::MAX, Ordering::Relaxed);
        PZ_MAX.store(0, Ordering::Relaxed);
        VTX_LOGGED.store(0, Ordering::Relaxed);
    }
    pub fn note_w(w: f32, onscreen: bool) {
        if !on() {
            return;
        }
        W_TOTAL.fetch_add(1, Ordering::Relaxed);
        if onscreen {
            W_ONSCREEN.fetch_add(1, Ordering::Relaxed);
        }
        if !w.is_finite() || w.abs() > 1.0e5 {
            W_PATHOLOGICAL.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn summary() {
        if !on() {
            return;
        }
        let t = W_TOTAL.load(Ordering::Relaxed);
        let on = W_ONSCREEN.load(Ordering::Relaxed);
        let path = W_PATHOLOGICAL.load(Ordering::Relaxed);
        if t > 0 {
            let pzmin = (PZ_MIN.load(Ordering::Relaxed) as i64 - (1i64 << 40)) as f64 / 1000.0;
            let pzmax = (PZ_MAX.load(Ordering::Relaxed) as i64 - (1i64 << 40)) as f64 / 1000.0;
            eprintln!(
                "[FN64_DUMP_PROJ] SUMMARY: {t} projected vtx | on-screen NDC-cube: {on} ({:.1}%) | pathological |w|>1e5 or non-finite: {path} ({:.1}%) | screen-z(pz) range [{pzmin:.2}, {pzmax:.2}] (nearer=smaller, z-test is `z<depth`)",
                100.0 * on as f64 / t as f64,
                100.0 * path as f64 / t as f64
            );
        }
    }
}

fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (r, out_row) in out.iter_mut().enumerate() {
        for (c, out_cell) in out_row.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            *out_cell = s;
        }
    }
    out
}

/// Transform a homogeneous point (x,y,z,1) by `m` using the N64's ROW-VECTOR
/// convention: `clip = v_row · m`, i.e. `out[c] = sum_r v[r] * m[r][c]`.
///
/// The N64 RSP treats vertices as row vectors and matrices in hardware
/// `[row][col]` layout (`clip = v · M · V · P`); `read_mtx` stores each `Mtx`
/// element at `m[row][col]` with NO transpose, and `recompute_mvp` composes
/// `mvp = M · (V · P)` in that same layout. The homogeneous point must
/// therefore be applied on the LEFT as a row vector. Applying it on the RIGHT
/// as a column vector (`m · v`, the old code) computes `mvp^T · v` -- the
/// TRANSPOSE of the true transform. For the perspective MVP that put the
/// projective term (`m[2][3] = -1`) into the OUTPUT ROW instead of the w
/// column, so `w` became `m[3][0]·x + m[3][1]·y + m[3][2]·z` (a huge,
/// sign-flipping value ~±thousands for ob coords of only ±10) instead of the
/// depth `-z_eye`. That is the "giant triangles fanning from a point" bug --
/// vertices with |w|≈thousands and random sign perspective-divide to garbage.
/// Verified against a live OoT gameplay task's decoded P (persp row
/// `[0,0,-1.0016,-1]`) + modelview translation `[-53,-5,0,1]`: column-vector
/// gave `w=-1531.75`; row-vector gives `w=5.0` (= `-z_eye`).
///
/// For a symmetric/diagonal matrix (all the reference-fixture cases exercise)
/// `m == m^T`, so this is identical to the old column-vector product -- the
/// fixture goldens are unchanged. Only the real perspective·view·model
/// product (asymmetric) is affected, which is exactly the gameplay path.
fn transform_point(m: &Mat4, x: f32, y: f32, z: f32) -> [f32; 4] {
    let v = [x, y, z, 1.0];
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let mut s = 0.0;
        for r in 0..4 {
            s += v[r] * m[r][c];
        }
        out[c] = s;
    }
    out
}

/// Read an N64 fixed-point `Mtx` (64 bytes) at `addr` out of `rdram` and
/// convert to an f32 `Mat4`. The N64 `Mtx` layout (gbi.h `Mtx` union,
/// documented public format): the first 32 bytes hold each element's signed
/// integer part as a big-endian s16; the next 32 bytes hold each element's
/// fractional part as a big-endian u16. The real value is
/// `int_part + frac_part / 65536`. Elements are stored row-major
/// (`m[4][4]`). Returns `None` if the 64-byte read would run off `rdram`.
///
/// We store the element (r,c) at `m[r][c]` -- the SAME `[row][col]` layout
/// the hardware `Mtx` (and RT64's `FixedMatrix::toMatrix4x4`) uses, with NO
/// transpose. The N64's row-vector convention (`clip = v_row * M`) is then
/// reproduced by composing the model/view/projection product in hardware
/// order (`recompute_mvp`) and applying it to the vertex as a ROW vector in
/// `transform_point` (`clip = v_row · mvp`). Applying the composed matrix as
/// a COLUMN vector instead (`mvp · v`) computes `mvp^T · v` -- the TRANSPOSE
/// of the true transform -- which put the perspective term into the output
/// row instead of the w column and made `w` a huge sign-flipping value; see
/// `transform_point`'s doc for the cited P/M numbers.
fn read_mtx(rdram: &[u8], addr: usize) -> Option<Mat4> {
    if addr + 64 > rdram.len() {
        return None;
    }
    let mut m = [[0.0f32; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let elem = r * 4 + c;
            let int_off = addr + elem * 2;
            let frac_off = addr + 32 + elem * 2;
            // Swizzled halfword reads (recomp MEM_H): the Mtx was DMA'd from
            // ROM through the same `^3` per-byte swizzle as everything else.
            let int_part = read_i16(rdram, int_off) as i32;
            let frac_part = read_u16(rdram, frac_off) as i32;
            let value = (((int_part << 16) | frac_part) as f32) / 65536.0;
            // Natural row-major store (hardware [row][col]): NO transpose.
            *cell = value;
        }
    }
    Some(m)
}

/// Read an N64 `Vp` (viewport) struct (16 bytes) at `addr` out of `rdram`
/// and convert to a pixel-space [`Viewport`]. Layout (F3DEX2-CONCEPTS.md
/// §1.4/§3.5): 8 big-endian s16 -- `vscale[4]` (x, y, z, w) then
/// `vtrans[4]` (x, y, z, w), each in the N64 "quarter-pixel" encoding
/// (÷4 for pixel units). Reads through the recomp `^3`/`MEM_H` swizzle
/// like every other DMA'd struct. Returns `None` if the 16-byte read runs
/// off `rdram`.
fn read_viewport(rdram: &[u8], addr: usize) -> Option<Viewport> {
    if addr + 16 > rdram.len() {
        return None;
    }
    let vscale_x = read_i16(rdram, addr) as f32;
    let vscale_y = read_i16(rdram, addr + 2) as f32;
    let vscale_z = read_i16(rdram, addr + 4) as f32;
    // addr+6 = vscale.w (unused for screen mapping)
    let vtrans_x = read_i16(rdram, addr + 8) as f32;
    let vtrans_y = read_i16(rdram, addr + 10) as f32;
    let vtrans_z = read_i16(rdram, addr + 12) as f32;
    // addr+14 = vtrans.w (unused)
    let vp = Viewport {
        sx: vscale_x / 4.0,
        sy: vscale_y / 4.0,
        sz: vscale_z / 4.0,
        tx: vtrans_x / 4.0,
        ty: vtrans_y / 4.0,
        tz: vtrans_z / 4.0,
    };
    #[cfg(not(test))]
    if crate::debug_flag("FN64_DUMP_PROJ") {
        eprintln!(
            "[FN64_DUMP_PROJ] viewport: sz={} tz={} => screen-z range [{}, {}] (near->far)",
            vp.sz,
            vp.tz,
            -vp.sz + vp.tz,
            vp.sz + vp.tz
        );
    }
    Some(vp)
}

// --- Vertex lighting (F3DEX2-CONCEPTS.md §2.4) --------------------------

/// Read a `Light_t` (16 bytes, OoT gbi.h:1311 -- `col[3]` u8, pad, `colc[3]`
/// u8, pad, `dir[3]` s8, pad) out of `rdram` at `addr` and install it into
/// light `slot`. Directional slots keep both direction (unit, s8÷127) and
/// color; the ambient slot (`slot == num_dir`) has no meaningful direction,
/// so we ALSO copy its color into `ambient` -- the RSP treats the highest
/// light as pure ambient regardless of its `dir` bytes (gbi.h:2902). Reads
/// through the recomp `^3`/`MEM_B` swizzle like every other DMA'd struct.
fn load_light(rdram: &[u8], state: &mut DecodeState, addr: usize, slot: usize) {
    assert!(
        slot < MAX_LIGHTS,
        "G_MOVEMEM G_MV_LIGHT destination slot {slot} exceeds slots 0..{}",
        MAX_LIGHTS - 1
    );
    let end = addr.checked_add(LIGHT_STRIDE).unwrap_or_else(|| {
        panic!("G_MOVEMEM G_MV_LIGHT source {addr:#x} overflows the host address space")
    });
    assert!(
        end <= rdram.len(),
        "G_MOVEMEM G_MV_LIGHT reads past RDRAM: source={addr:#x}, bytes={LIGHT_STRIDE}, rdram_bytes={}",
        rdram.len()
    );
    // col[0..3] at bytes 0..3; dir[3] (s8) at bytes 8..11.
    let cr = read_u8(rdram, addr) as f32 / 255.0;
    let cg = read_u8(rdram, addr + 1) as f32 / 255.0;
    let cb = read_u8(rdram, addr + 2) as f32 / 255.0;
    // dir is signed s8 ÷127 -> a (roughly) unit direction (RSPProcessCS.hlsl
    // `srcNorm / 127`).
    let dx = (read_u8(rdram, addr + 8) as i8) as f32 / 127.0;
    let dy = (read_u8(rdram, addr + 9) as i8) as f32 / 127.0;
    let dz = (read_u8(rdram, addr + 10) as i8) as f32 / 127.0;
    state.lights.dir[slot] = DirLight {
        dir: [dx, dy, dz],
        col: [cr, cg, cb],
    };
    // If this slot is the ambient slot (the one just past the directional
    // count), mirror its color into `ambient`.
    if slot == state.lights.num_dir {
        state.lights.ambient = [cr, cg, cb];
    }
}

/// One destination in the public two-entry `LookAt` structure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LookAtAxis {
    X,
    Y,
}

/// F3DEX2 automatic texture-coordinate projection state. Each direction is
/// absent until its corresponding `gSPLookAtX`/`gSPLookAtY` DMA is observed;
/// texture generation cannot manufacture a usable default because the public
/// helpers derive both directions from the active eye/object orientation.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct LookAtState {
    x: Option<[f32; 3]>,
    y: Option<[f32; 3]>,
}

/// Decode the direction bytes in one public `Light_t`-shaped LookAt entry.
/// `gdSPDefLookAt` and `guLookAtReflect` place the signed screen-space
/// direction in bytes 8..10; the color bytes are placeholders.
fn load_look_at(rdram: &[u8], state: &mut DecodeState, addr: usize, axis: LookAtAxis) {
    let end = addr
        .checked_add(LIGHT_STRIDE)
        .expect("G_MOVEMEM gSPLookAt source range overflows host address space");
    assert!(
        end <= rdram.len(),
        "G_MOVEMEM gSPLookAt {axis:?} reads past RDRAM: source={addr:#x}, bytes={LIGHT_STRIDE}"
    );
    let direction = [
        (read_u8(rdram, addr + 8) as i8) as f32 / 127.0,
        (read_u8(rdram, addr + 9) as i8) as f32 / 127.0,
        (read_u8(rdram, addr + 10) as i8) as f32 / 127.0,
    ];
    match axis {
        LookAtAxis::X => state.look_at.x = Some(direction),
        LookAtAxis::Y => state.look_at.y = Some(direction),
    }
}

/// Decode one of the public F3DEX2 `G_MWO_{a,b}LIGHT_n` destinations.
/// Each light occupies 24 bytes in the microcode state table; the primary and
/// copied colors are the words at offsets 0 and 4 within that stride.
fn light_slot_from_moveword_offset(offset: u16) -> Option<usize> {
    let stride_offset = usize::from(offset);
    let word = stride_offset % 24;
    if !matches!(word, 0 | 4) {
        return None;
    }
    let slot = stride_offset / 24;
    (slot < MAX_LIGHTS).then_some(slot)
}

fn set_light_color(state: &mut DecodeState, slot: usize, rgba: u32) {
    let [r, g, b, _alpha] = rgba.to_be_bytes();
    let color = [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    ];
    state.lights.dir[slot].col = color;
    if slot == state.lights.num_dir {
        state.lights.ambient = color;
    }
}

/// Decode the F3DEX2 light slot selected by a `G_MOVEMEM G_MV_LIGHT`
/// destination offset. `gSPLight(..., n)` emits `(n * 24 + 24) / 8` in the
/// wire field, while DMEM indices 0 and 1 are reserved for the two look-at
/// vectors. Therefore `LIGHT_1` starts at DMEM index 2 and maps to light slot
/// 0, matching RT64's `offset / 24 - 2` dispatch.
fn light_slot_from_movemem_offset(ofs_div8: usize) -> Option<usize> {
    #[cfg(not(test))]
    let reserved_slots = if std::env::var_os("FN64_DIAG_OLD_LIGHT_SLOT").is_some() {
        1
    } else {
        2
    };
    #[cfg(test)]
    let reserved_slots = 2;
    (ofs_div8 / 3)
        .checked_sub(reserved_slots)
        .filter(|&slot| slot < MAX_LIGHTS)
}

/// Normalize a 3-vector; returns the zero vector unchanged (guards a 0-length
/// normal/direction so a bad DMA can't produce NaN).
#[inline]
fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Rotate a direction (w=0) by the 3x3 upper-left of a `Mat4` (row-major,
/// column-vector convention like `transform_point`). Used to bring a light
/// direction from world/eye space into the vertex's local space, matching
/// RT64's `computeDirLight` (`mul(float4(dir,0), worldMat)`), which multiplies
/// by the modelview so N·L is evaluated in the same space as the (untransformed)
/// vertex normal.
#[inline]
fn rotate_dir(m: &Mat4, d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[r][0] * d[0] + m[r][1] * d[1] + m[r][2] * d[2];
    }
    out
}

/// Compute a lit vertex color from a NORMAL (`cn` reinterpreted as s8÷127),
/// the loaded lights, and the current modelview (light-space transform).
/// Ambient + Σ over directionals of `max(N·L, 0) * lightColor`, clamped to
/// [0,1] per channel, returned as u8 RGB. This mirrors RT64's
/// `RSPProcessCS.hlsl` lighting branch (ambient is the base, each directional
/// adds `computeDirLight`, result `min(.,1)`), the microcode-faithful model.
fn light_vertex(state: &DecodeState, normal: [f32; 3]) -> [u8; 3] {
    let n = normalize3(normal);
    let mut c = state.lights.ambient;
    for i in 0..state.lights.num_dir {
        let light = &state.lights.dir[i];
        // Bring the light direction into the vertex's (model) space via the
        // modelview, normalize, then N·L (clamped at 0 -- unlit back side
        // contributes nothing).
        let ld = normalize3(rotate_dir(&state.modelview, light.dir));
        let ndotl = (n[0] * ld[0] + n[1] * ld[1] + n[2] * ld[2]).max(0.0);
        c[0] += ndotl * light.col[0];
        c[1] += ndotl * light.col[1];
        c[2] += ndotl * light.col[2];
    }
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    ]
}

/// Generate texture coordinates from the public reflection-mapping contract
/// (Programming Manual 11.7.5). Regular mode maps each signed look-at
/// projection from [-1,+1] to [0,scale]. Linear mode maps `acos(projection)`
/// from [0,pi] to [0,scale]. The intermediate 32768 range is S10.5 texture
/// coordinate space before the U0.16 `gSPTexture` scale and `/32` texel
/// conversion already used for explicit vertex coordinates.
fn generated_texture_coords(state: &DecodeState, normal: [f32; 3]) -> (f32, f32) {
    assert_ne!(
        state.geometry_mode & G_LIGHTING,
        0,
        "G_TEXTURE_GEN requires G_LIGHTING so vertex cn bytes are normals"
    );
    let look_x = state
        .look_at
        .x
        .expect("G_TEXTURE_GEN requires a preceding gSPLookAtX DMA");
    let look_y = state
        .look_at
        .y
        .expect("G_TEXTURE_GEN requires a preceding gSPLookAtY DMA");

    let n = normalize3(normal);
    let x = normalize3(rotate_dir(&state.modelview, look_x));
    let y = normalize3(rotate_dir(&state.modelview, look_y));
    let project =
        |axis: [f32; 3]| (n[0] * axis[0] + n[1] * axis[1] + n[2] * axis[2]).clamp(-1.0, 1.0);
    let linear = state.geometry_mode & G_TEXTURE_GEN_LINEAR != 0;
    let generated_raw = |projection: f32| {
        if linear {
            projection.acos() / std::f32::consts::PI * 32768.0
        } else {
            (projection + 1.0) * 16384.0
        }
    };
    (
        generated_raw(project(x)) * state.tex.tex_scale_s / 32.0,
        generated_raw(project(y)) * state.tex.tex_scale_t / 32.0,
    )
}

// --- Texture format decode (F3DEX2-CONCEPTS.md §5.1) --------------------
//
// Format/size selector values: OoT `include/ultra64/gbi.h:331-378`.
// Texel bit layouts and channel expansion: RT64 (MIT)
// `src/shaders/Formats.hlsli:56-119` and
// `src/shaders/TextureDecoder.hlsli:30-120,149-204`.

/// RDP image formats (`G_IM_FMT_*`) as encoded in the SETTIMG/SETTILE
/// format field.
const G_IM_FMT_RGBA: u8 = 0;
const G_IM_FMT_YUV: u8 = 1;
const G_IM_FMT_CI: u8 = 2;
const G_IM_FMT_IA: u8 = 3;
const G_IM_FMT_I: u8 = 4;

/// Pixel sizes (`G_IM_SIZ_*`): 4/8/16/32 bits-per-texel selectors.
const G_IM_SIZ_4B: u8 = 0;
const G_IM_SIZ_8B: u8 = 1;
const G_IM_SIZ_16B: u8 = 2;
const G_IM_SIZ_32B: u8 = 3;

/// Expand a 16-bit RGBA5551 texel to RGBA8888 (5/5/5/1, big-endian).
/// RT64 `Formats.hlsli:83-92` gives the exact shifts and 5-to-8 replication;
/// OoT `gbi.h:334,345` identifies this as `G_IM_FMT_RGBA/G_IM_SIZ_16b`.
#[inline]
fn rgba5551_to_rgba8888(px: u16) -> [u8; 4] {
    let r5 = ((px >> 11) & 0x1F) as u8;
    let g5 = ((px >> 6) & 0x1F) as u8;
    let b5 = ((px >> 1) & 0x1F) as u8;
    let a1 = (px & 0x01) as u8;
    // 5-bit -> 8-bit: replicate high bits into the low bits (v<<3 | v>>2).
    let expand5 = |v: u8| (v << 3) | (v >> 2);
    [
        expand5(r5),
        expand5(g5),
        expand5(b5),
        if a1 != 0 { 255 } else { 0 },
    ]
}

/// Expand IA16 (8-bit intensity, 8-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:108-111` (`gbi.h:337,345`).
#[inline]
fn ia16_to_rgba8888(hi: u8, lo: u8) -> [u8; 4] {
    [hi, hi, hi, lo]
}

/// Expand IA8 (4-bit intensity, 4-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:75-80` (`gbi.h:337,344`).
#[inline]
fn ia8_to_rgba8888(byte: u8) -> [u8; 4] {
    let i4 = byte >> 4;
    let a4 = byte & 0x0F;
    let i = (i4 << 4) | i4;
    let a = (a4 << 4) | a4;
    [i, i, i, a]
}

/// Expand IA4 (3-bit intensity, 1-bit alpha) to RGBA8888, matching RT64
/// `Formats.hlsli:61-64` (`gbi.h:337,343`).
#[inline]
fn ia4_to_rgba8888(nibble: u8) -> [u8; 4] {
    let i3 = (nibble >> 1) & 0x07;
    // Exact 3-to-8 replication: abc -> abcabcab.
    let i = (i3 << 5) | (i3 << 2) | (i3 >> 1);
    [i, i, i, if nibble & 1 != 0 { 255 } else { 0 }]
}

/// Expand I8 (8-bit intensity; alpha = intensity) to RGBA8888, matching
/// RT64 `Formats.hlsli:71-73` (`gbi.h:338,344`).
#[inline]
fn i8_to_rgba8888(byte: u8) -> [u8; 4] {
    [byte, byte, byte, byte]
}

/// Expand I4 (4-bit intensity; alpha = intensity) to RGBA8888, matching
/// RT64 `Formats.hlsli:56-59` (`gbi.h:338,343`).
#[inline]
fn i4_to_rgba8888(nibble: u8) -> [u8; 4] {
    let i = (nibble << 4) | nibble;
    [i, i, i, i]
}

/// Select one 4-bit texel from a packed byte. RT64
/// `TextureDecoder.hlsli:170-172` selects the high nibble for even columns
/// and the low nibble for odd columns.
#[inline]
fn packed_nibble(byte: u8, texel_index: usize) -> u8 {
    if texel_index & 1 == 0 {
        byte >> 4
    } else {
        byte & 0x0F
    }
}

/// Decode `G_LOADTLUT`'s 10-bit count field. Public `gbi.h` packs
/// `count - 1` directly at bits 14..23; the low two bits are part of the
/// count, not fixed-point padding.
fn load_tlut_count(w1: u32) -> usize {
    let count = ((w1 >> 14) & 0x3ff) as usize + 1;
    assert!(
        count <= 256,
        "G_LOADTLUT requested {count} entries, exceeding the 256-entry TLUT"
    );
    count
}

#[cfg(test)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TextureLoad {
    Block,
    Tile { source_x: u32, source_y: u32 },
}

fn source_texel(rdram: &[u8], base: usize, index: usize, size: u8) -> u32 {
    match size {
        G_IM_SIZ_4B => {
            let byte = read_u8(rdram, base + index / 2);
            u32::from(packed_nibble(byte, index))
        }
        G_IM_SIZ_8B => u32::from(read_u8(rdram, base + index)),
        G_IM_SIZ_16B => u32::from(read_u16(rdram, base + index * 2)),
        G_IM_SIZ_32B => {
            let offset = base + index * 4;
            u32::from_be_bytes([
                read_u8(rdram, offset),
                read_u8(rdram, offset + 1),
                read_u8(rdram, offset + 2),
                read_u8(rdram, offset + 3),
            ])
        }
        _ => unreachable!("RDP image size is a two-bit field"),
    }
}

fn assert_texture_source_range(
    rdram: &[u8],
    base: usize,
    last_index: usize,
    size: u8,
    command: &str,
) {
    let last_byte = match size {
        G_IM_SIZ_4B => last_index / 2,
        G_IM_SIZ_8B => last_index,
        G_IM_SIZ_16B => last_index * 2 + 1,
        G_IM_SIZ_32B => last_index * 4 + 3,
        _ => unreachable!("RDP image size is a two-bit field"),
    };
    assert!(
        base.checked_add(last_byte)
            .is_some_and(|end| end < rdram.len()),
        "{command} source texel {last_index} exceeds RDRAM length {:#x}",
        rdram.len()
    );
}

fn load_tile_into_tmem(
    rdram: &[u8],
    tex: &mut TexState,
    segments: &[u32; 16],
    tile_index: usize,
    w0: u32,
    w1: u32,
) {
    let raw_source_x = ((w0 >> 12) & 0x0fff) as usize;
    let raw_source_y = (w0 & 0x0fff) as usize;
    let raw_high_x = ((w1 >> 12) & 0x0fff) as usize;
    let raw_high_y = (w1 & 0x0fff) as usize;
    // SGI RDP Command Summary Table 7 says equal L/H fractions are the usual
    // subpixel-offset form, not a validity requirement. The integer parts
    // select the inclusive DRAM texel span while all raw quarters remain in
    // the tile descriptor for later sampling/clamping.
    let source_x = raw_source_x / 4;
    let source_y = raw_source_y / 4;
    let high_x = raw_high_x / 4;
    let high_y = raw_high_y / 4;
    assert!(
        high_x >= source_x && high_y >= source_y,
        "G_LOADTILE has inverted source bounds ({source_x}, {source_y})..=({high_x}, {high_y})"
    );
    assert_ne!(
        tex.timg_width, 0,
        "G_LOADTILE decoded before G_SETTIMG latched a source width"
    );
    let width = high_x - source_x + 1;
    let height = high_y - source_y + 1;
    let base = resolve_addr(segments, tex.timg_addr);
    let last_index = (high_y * usize::from(tex.timg_width)) + high_x;
    assert_texture_source_range(rdram, base, last_index, tex.timg_siz, "G_LOADTILE");
    let tile = tex.tiles[tile_index];
    if tex.timg_fmt == G_IM_FMT_YUV {
        assert_eq!(
            (tile.fmt, tile.siz),
            (G_IM_FMT_YUV, G_IM_SIZ_16B),
            "YUV G_LOADTILE requires a YUV16 load descriptor"
        );
        assert!(
            source_x.is_multiple_of(2) && width.is_multiple_of(2),
            "YUV G_LOADTILE requires an even S origin and width"
        );
        for y in 0..height {
            let source_row = source_y + y;
            for pair in 0..width / 2 {
                let index = source_row * usize::from(tex.timg_width) + source_x + pair * 2;
                let offset = base + index * 2;
                std::rc::Rc::make_mut(&mut tex.tmem).write_yuv_pair(
                    tile,
                    pair,
                    y,
                    source_row & 1 != 0,
                    [
                        read_u8(rdram, offset),
                        read_u8(rdram, offset + 1),
                        read_u8(rdram, offset + 2),
                        read_u8(rdram, offset + 3),
                    ],
                );
            }
        }
    } else {
        for y in 0..height {
            let source_row = source_y + y;
            for x in 0..width {
                let index = source_row * usize::from(tex.timg_width) + source_x + x;
                let value = source_texel(rdram, base, index, tex.timg_siz);
                // The same Table 7 usage notes allow image and load-tile
                // sizes to differ. DRAM addressing and transferred bit width
                // belong to G_SETTIMG; the tile still owns TMEM base/line.
                std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
                    tile,
                    x,
                    y,
                    source_row & 1 != 0,
                    tex.timg_siz,
                    value,
                );
            }
        }
    }

    let tile = &mut tex.tiles[tile_index];
    tile.uls = ((w0 >> 12) & 0x0fff) as u16;
    tile.ult = (w0 & 0x0fff) as u16;
    tile.lrs = ((w1 >> 12) & 0x0fff) as u16;
    tile.lrt = (w1 & 0x0fff) as u16;
}

fn load_block_into_tmem(
    rdram: &[u8],
    tex: &mut TexState,
    segments: &[u32; 16],
    tile_index: usize,
    w0: u32,
    w1: u32,
) {
    let source_s = ((w0 >> 12) & 0x0fff) as usize;
    let source_t = (w0 & 0x0fff) as usize;
    let high_s = ((w1 >> 12) & 0x0fff) as usize;
    let dxt = (w1 & 0x0fff) as usize;
    assert!(
        high_s >= source_s,
        "G_LOADBLOCK has inverted source span {source_s}..={high_s}"
    );
    assert_ne!(
        tex.timg_width, 0,
        "G_LOADBLOCK decoded before G_SETTIMG latched a source width"
    );

    let count = high_s - source_s + 1;
    let start = source_t * usize::from(tex.timg_width) + source_s;
    let base = resolve_addr(segments, tex.timg_addr);
    assert_texture_source_range(rdram, base, start + count - 1, tex.timg_siz, "G_LOADBLOCK");
    let tile = tex.tiles[tile_index];
    if tex.timg_fmt == G_IM_FMT_YUV {
        assert_eq!(
            (tile.fmt, tile.siz),
            (G_IM_FMT_YUV, G_IM_SIZ_16B),
            "YUV G_LOADBLOCK requires a YUV16 load descriptor"
        );
        assert!(
            start.is_multiple_of(2) && count.is_multiple_of(2),
            "YUV G_LOADBLOCK requires an even source origin and texel count"
        );
        for offset in (0..count).step_by(2) {
            let word = offset / 8;
            let t_advance = (word * dxt) >> 11;
            let destination_word =
                usize::from(tile.tmem) + word + t_advance * usize::from(tile.line);
            let destination = Tile {
                tmem: (destination_word & 0x01ff) as u16,
                line: 0,
                ..tile
            };
            let source_offset = base + (start + offset) * 2;
            std::rc::Rc::make_mut(&mut tex.tmem).write_yuv_pair(
                destination,
                (offset % 8) / 2,
                0,
                (source_t + t_advance) & 1 != 0,
                [
                    read_u8(rdram, source_offset),
                    read_u8(rdram, source_offset + 1),
                    read_u8(rdram, source_offset + 2),
                    read_u8(rdram, source_offset + 3),
                ],
            );
        }
    } else {
        // Table 8 defines DXT stepping per transferred 64-bit word, so the
        // number of command texels per word comes from the source image size,
        // not a deliberately mismatched load descriptor.
        let texels_per_word = 16usize >> tex.timg_siz;
        for offset in 0..count {
            let word = offset / texels_per_word;
            let t_advance = (word * dxt) >> 11;
            let destination_word =
                usize::from(tile.tmem) + word + t_advance * usize::from(tile.line);
            let destination = Tile {
                tmem: (destination_word & 0x01ff) as u16,
                line: 0,
                ult: ((source_t + t_advance) as u16).wrapping_mul(4),
                ..tile
            };
            let value = source_texel(rdram, base, start + offset, tex.timg_siz);
            std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
                destination,
                offset % texels_per_word,
                0,
                (source_t + t_advance) & 1 != 0,
                tex.timg_siz,
                value,
            );
        }
    }

    let tile = &mut tex.tiles[tile_index];
    tile.uls = (source_s as u16) & 0x0fff;
    tile.ult = (source_t as u16) & 0x0fff;
    tile.lrs = (high_s as u16) & 0x0fff;
    tile.lrt = (dxt as u16) & 0x0fff;
}

#[cfg(test)]
fn palette_color(tlut: &[[u8; 4]], index: usize, format: &str) -> [u8; 4] {
    *tlut.get(index).unwrap_or_else(|| {
        panic!(
            "{format} texel index {index} exceeds the loaded {}-entry TLUT",
            tlut.len()
        )
    })
}

/// Test-only direct decoder retained as a format-conversion oracle. Production
/// display-list execution loads and samples physical [`Tmem`] instead.
/// Decode the texture bound to `tile` from the latched `G_SETTIMG` image out
/// of RDRAM into an RGBA8888 [`Texture`], sized by the tile's
/// `G_SETTILESIZE` extent. Unsupported dimensions or formats trap by their
/// decoded fields; a texture request must never degrade into flat shading.
/// Covers the common OoT formats: RGBA16/32, RGBA4/8 hardware
/// aliases, IA16/IA8/IA4, I8/I4, and CI8/CI4 (via the loaded TLUT).
///
/// This helper deliberately bypasses TMEM so individual conversion tests can
/// isolate one source format without constructing a command stream.
#[cfg(test)]
fn decode_current_texture(
    rdram: &[u8],
    tex: &TexState,
    segments: &[u32; 16],
    tile: usize,
    load: TextureLoad,
) -> Texture {
    let t = &tex.tiles[tile];
    // Tile extent from SETTILESIZE (S10.5 -> ÷4 texels), inclusive bounds.
    let (uls, ult, lrs, lrt) = (t.uls / 4, t.ult / 4, t.lrs / 4, t.lrt / 4);
    assert!(
        lrs >= uls && lrt >= ult,
        "texture tile {tile} has reversed extent ({uls}, {ult})..({lrs}, {lrt})"
    );
    let w = u32::from(lrs - uls + 1);
    let h = u32::from(lrt - ult + 1);
    assert!(
        w != 0 && h != 0 && w <= 1024 && h <= 1024,
        "texture tile {tile} has unsupported extent {w}x{h}"
    );
    let base = resolve_addr(segments, tex.timg_addr);
    let fmt = t.fmt;
    let siz = t.siz;
    let mut texels = vec![0u8; (w * h * 4) as usize];
    if matches!(load, TextureLoad::Tile { .. }) {
        assert_ne!(
            tex.timg_width, 0,
            "G_LOADTILE decoded before G_SETTIMG latched a source width"
        );
    }

    for ty in 0..h {
        for tx in 0..w {
            let texel_index = (ty * w + tx) as usize;
            let source_index = match load {
                TextureLoad::Block => texel_index,
                TextureLoad::Tile { source_x, source_y } => {
                    ((source_y + ty) * u32::from(tex.timg_width) + source_x + tx) as usize
                }
            };
            let rgba = match (fmt, siz) {
                (G_IM_FMT_RGBA, G_IM_SIZ_16B) => {
                    let px = read_u16(rdram, base + source_index * 2);
                    rgba5551_to_rgba8888(px)
                }
                (G_IM_FMT_RGBA, G_IM_SIZ_32B) => {
                    let o = base + source_index * 4;
                    [
                        read_u8(rdram, o),
                        read_u8(rdram, o + 1),
                        read_u8(rdram, o + 2),
                        read_u8(rdram, o + 3),
                    ]
                }
                (G_IM_FMT_YUV, G_IM_SIZ_16B) => {
                    // SGI RDP Command Summary, Set Tile/Load Tile notes:
                    // YUV images are byte-interleaved Y0,U,Y1,V and each
                    // adjacent Y pair shares its U/V chroma samples.
                    let pair = source_index / 2;
                    let pair_base = base + pair * 4;
                    let y_offset = if source_index & 1 == 0 { 0 } else { 2 };
                    [
                        read_u8(rdram, pair_base + y_offset),
                        read_u8(rdram, pair_base + 1),
                        read_u8(rdram, pair_base + 3),
                        255,
                    ]
                }
                (G_IM_FMT_IA, G_IM_SIZ_16B) => {
                    let o = base + source_index * 2;
                    ia16_to_rgba8888(read_u8(rdram, o), read_u8(rdram, o + 1))
                }
                (G_IM_FMT_IA, G_IM_SIZ_8B) => ia8_to_rgba8888(read_u8(rdram, base + source_index)),
                (G_IM_FMT_I, G_IM_SIZ_8B) | (G_IM_FMT_RGBA, G_IM_SIZ_8B) => {
                    // RGBA8 is not a nominal GBI format, but RT64's observed
                    // hardware path samples it identically to I8
                    // (`TextureDecoder.hlsli:68-75`).
                    i8_to_rgba8888(read_u8(rdram, base + source_index))
                }
                (G_IM_FMT_IA, G_IM_SIZ_4B) => {
                    let byte = read_u8(rdram, base + source_index / 2);
                    ia4_to_rgba8888(packed_nibble(byte, source_index))
                }
                (G_IM_FMT_I, G_IM_SIZ_4B) | (G_IM_FMT_RGBA, G_IM_SIZ_4B) => {
                    // RGBA4 likewise aliases I4 on hardware (RT64
                    // `TextureDecoder.hlsli:45-56`). OoT's real 250-swap
                    // C-boot trace exercises this otherwise-unsupported pair.
                    let byte = read_u8(rdram, base + source_index / 2);
                    i4_to_rgba8888(packed_nibble(byte, source_index))
                }
                (G_IM_FMT_CI, G_IM_SIZ_8B) => {
                    // RT64 `TextureDecoder.hlsli:174-184`: an 8-bit CI texel
                    // is the full TLUT index. OoT uses RGBA16 TLUTs only
                    // (`oot-decomp/docs/assets/images.md:63-64`).
                    let idx = read_u8(rdram, base + source_index) as usize;
                    palette_color(&tex.tlut, idx, "CI8")
                }
                (G_IM_FMT_CI, G_IM_SIZ_4B) => {
                    let byte = read_u8(rdram, base + source_index / 2);
                    // RT64 `TextureDecoder.hlsli:176-179`: CI4 prepends the
                    // tile's four-bit palette bank to the texel nibble in
                    // TMEM. A 16-entry G_LOADTLUT is stored by this decoder as
                    // a palette-local Vec (entry zero is that bank's first
                    // color), while a full TLUT remains globally indexed.
                    let nib = packed_nibble(byte, source_index) as usize;
                    let idx = if tex.tlut.len() <= 16 {
                        nib
                    } else {
                        ((t.palette as usize) << 4) | nib
                    };
                    palette_color(&tex.tlut, idx, "CI4")
                }
                _ => panic!("texture tile {tile} uses unsupported format {fmt} size {siz}"),
            };
            let o = texel_index * 4;
            texels[o..o + 4].copy_from_slice(&rgba);
        }
    }

    Texture {
        format: t.fmt,
        size: t.siz,
        width: w,
        height: h,
        texels: std::rc::Rc::new(texels),
        clamp_s: t.clamp_s,
        clamp_t: t.clamp_t,
        mirror_s: t.mirror_s,
        mirror_t: t.mirror_t,
        mask_s: t.mask_s,
        mask_t: t.mask_t,
        shift_s: t.shift_s,
        shift_t: t.shift_t,
        origin_s: t.uls as f32 / 4.0,
        origin_t: t.ult as f32 / 4.0,
        tmem: None,
        lod: None,
    }
}

/// Human-readable name for command diagnostics.
fn opcode_name(opcode: u8) -> &'static str {
    match opcode {
        G_NOOP => "G_NOOP",
        G_VTX => "G_VTX",
        G_MODIFYVTX => "G_MODIFYVTX",
        G_CULLDL => "G_CULLDL",
        G_BRANCH_Z => "G_BRANCH_Z",
        G_TRI1 => "G_TRI1",
        G_TRI2 => "G_TRI2",
        G_QUAD => "G_QUAD",
        G_LINE3D => "G_LINE3D",
        G_TEXRECT => "G_TEXRECT",
        G_TEXRECTFLIP => "G_TEXRECTFLIP",
        G_POPMTX => "G_POPMTX",
        G_MTX => "G_MTX",
        G_MOVEWORD => "G_MOVEWORD",
        G_DL => "G_DL",
        G_ENDDL => "G_ENDDL",
        G_SPNOOP => "G_SPNOOP",
        0xE1 => "G_RDPHALF_1",
        G_SETOTHERMODE_L => "G_SETOTHERMODE_L",
        G_SETOTHERMODE_H => "G_SETOTHERMODE_H",
        G_RDPLOADSYNC => "G_RDPLOADSYNC",
        G_RDPPIPESYNC => "G_RDPPIPESYNC",
        G_RDPTILESYNC => "G_RDPTILESYNC",
        G_RDPFULLSYNC => "G_RDPFULLSYNC",
        G_RDPSETOTHERMODE => "G_RDPSETOTHERMODE",
        G_SETKEYGB => "G_SETKEYGB",
        G_SETKEYR => "G_SETKEYR",
        G_SETCONVERT => "G_SETCONVERT",
        G_SETPRIMDEPTH => "G_SETPRIMDEPTH",
        G_LOADTLUT => "G_LOADTLUT",
        0xF1 => "G_RDPHALF_2",
        G_LOADBLOCK => "G_LOADBLOCK",
        G_LOADTILE => "G_LOADTILE",
        G_SETTILESIZE => "G_SETTILESIZE",
        G_SETTILE => "G_SETTILE",
        G_FILLRECT => "G_FILLRECT",
        G_SETFILLCOLOR => "G_SETFILLCOLOR",
        G_SETFOGCOLOR => "G_SETFOGCOLOR",
        G_SETBLENDCOLOR => "G_SETBLENDCOLOR",
        G_SETCOMBINE => "G_SETCOMBINE",
        G_SETTIMG => "G_SETTIMG",
        G_SETPRIMCOLOR => "G_SETPRIMCOLOR",
        G_SETENVCOLOR => "G_SETENVCOLOR",
        G_SETSCISSOR => "G_SETSCISSOR",
        G_SETZIMG => "G_SETZIMG",
        G_SETCIMG => "G_SETCIMG",
        G_SPECIAL_1 => "G_SPECIAL_1",
        G_SPECIAL_2 => "G_SPECIAL_2",
        G_SPECIAL_3 => "G_SPECIAL_3",
        G_DMA_IO => "G_DMA_IO",
        G_LOAD_UCODE => "G_LOAD_UCODE",
        G_TEXTURE => "G_TEXTURE",
        G_GEOMETRYMODE => "G_GEOMETRYMODE",
        G_MOVEMEM => "G_MOVEMEM",
        _ => "G_<unrecognized>",
    }
}

// --- Recomp rdram memory model (swizzled) -------------------------------
//
// fn64's `rdram` is NOT a flat big-endian image. The N64Recomp memory
// macros (`refs/N64RecompSource/include/recomp.h:95-107`) store every
// aligned 32-bit word in HOST-NATIVE order (`MEM_W` = a bare
// `*(int32_t*)`, no byteswap) and reach sub-word bytes/halfwords through an
// address XOR (`MEM_B` uses `^3`, `MEM_H` uses `^2`) -- the standard
// "byteswap within a native word" trick that makes big-endian sub-word
// access work over a little-endian word array. The PI-DMA path
// (`fn64-runtime/src/rdram.rs:243` `dma_write_bytes`) writes cartridge
// bytes with the SAME per-byte `^3` swizzle, so EVERYTHING in rdram --
// CPU-built display lists AND DMA'd vertex/matrix data -- obeys this one
// model. A decoder that reads it as flat big-endian (the old
// `from_be_bytes`) gets each 32-bit word byte-reversed: OoT's first DL
// command `0xDE...` (G_DL) read flat-BE became `0x000001DE` (opcode
// `0x00`), so the whole list decoded as garbage and produced 0 triangles.
//
// These helpers read logical values THE WAY THE GAME DOES: an aligned word
// is a native-endian `u32` (== the logical big-endian word), and any
// byte/halfword within it is extracted by its logical position. This is
// exactly equivalent to `MEM_W` / `MEM_HU(^2)` / `MEM_BU(^3)`.

/// Read the logical big-endian 32-bit word at aligned byte `off`
/// (`off % 4 == 0` expected; misaligned reads still return the containing
/// word's native value, matching a `MEM_W` on a masked address). Returns 0
/// if the word runs past `rdram`.
#[inline]
fn read_u32(rdram: &[u8], off: usize) -> u32 {
    let Some(aligned) = complete_storage_word(rdram, off) else {
        return 0;
    };
    fn64_runtime::RdramView::from_storage(rdram).read_u32(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(aligned).expect("GBI RDRAM address exceeds u32"),
    ))
}

#[inline]
fn complete_storage_word(rdram: &[u8], off: usize) -> Option<usize> {
    let aligned = off & !3;
    aligned
        .checked_add(4)
        .filter(|&end| end <= rdram.len())
        .map(|_| aligned)
}

/// Read a logical byte at byte offset `off` (recomp `MEM_BU`: physical
/// index `off ^ 3`). Returns 0 past the end.
#[inline]
fn read_u8(rdram: &[u8], off: usize) -> u8 {
    if complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_u8(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Read a logical signed 16-bit halfword at byte offset `off` (recomp
/// `MEM_H`). The two logical bytes `off` (MSB) and `off+1` (LSB) are read
/// through the `^3` byte swizzle and recombined big-endian. Returns 0 past
/// the end.
#[inline]
fn read_i16(rdram: &[u8], off: usize) -> i16 {
    if !off.is_multiple_of(2) || complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_i16(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Read a logical unsigned 16-bit halfword at byte offset `off`.
#[inline]
fn read_u16(rdram: &[u8], off: usize) -> u16 {
    if !off.is_multiple_of(2) || complete_storage_word(rdram, off).is_none() {
        return 0;
    }
    fn64_runtime::RdramView::from_storage(rdram).read_u16(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(off).expect("GBI RDRAM address exceeds u32"),
    ))
}

/// Resolve a (possibly segmented) F3DEX2 address to a flat rdram byte
/// offset. The top byte is the segment number; the low 24 bits are the
/// offset within that segment. If a segment base was registered (via
/// `G_MOVEWORD`/`G_MW_SEGMENT`) it is added; segment 0 is the identity
/// (physical) segment on real hardware, so an unset segment resolves to its
/// low-24-bit offset unchanged -- which is also exactly what the pre-
/// existing non-segmented fixtures (segment byte 0x00, e.g. addr 0x1000)
/// rely on, keeping them working unchanged.
fn resolve_addr(segments: &[u32; 16], addr: u32) -> usize {
    let seg = ((addr >> 24) & 0x0F) as usize;
    let off = (addr & 0x00FF_FFFF) as usize;
    segments[seg] as usize + off
}

/// Decoder state carried across (possibly nested via `G_DL`) command
/// streams.
struct DecodeState {
    vtx_cache: [Vertex; 64],
    ops: Vec<RenderOp>,
    segments: [u32; 16],
    /// Projection * modelview, recomputed whenever either changes. `None`
    /// means "no transform loaded yet" -> vertices pass through as raw `ob`
    /// screen coords (preserves the pre-existing raw-coordinate fixtures).
    mvp: Option<Mat4>,
    /// Matrix staged by `G_MOVEMEM G_MV_MATRIX` until the public compound
    /// command's `G_MOVEWORD G_MW_FORCEMTX` marker makes it authoritative.
    pending_forced_mvp: Option<Mat4>,
    proj: Option<Mat4>,
    modelview: Mat4,
    mv_stack: Vec<Mat4>,
    /// Viewport scale/translate (screen mapping), if a `G_MOVEMEM` viewport
    /// was seen. Fields: `(sx, sy, sz, tx, ty, tz)` -- x/y map NDC to pixels,
    /// z maps NDC-z to the depth range (all already divided by 4 in
    /// `read_viewport`). Transformed vertices require this state: inventing a
    /// host-sized default would hide a missing `G_MV_VIEWPORT` DMA and map the
    /// same display list differently from hardware. With no matrix at all the
    /// raw `ob` coordinates retain the reference-fixture convention.
    viewport: Option<Viewport>,
    scissor: Option<ScissorRect>,
    /// Current F3DEX2 geometry mode (the `G_GEOMETRYMODE` accumulator). Its
    /// `G_CULL_FRONT`/`G_CULL_BACK` bits decide per-triangle culling.
    geometry_mode: u32,
    /// RDP other-mode H/L plus blend-alpha threshold. F3DEX2 partial updates
    /// mutate this shared state; each emitted triangle snapshots it.
    other_mode: OtherMode,
    /// RDP color-combiner + primitive/environment register state. This is
    /// independent of other-mode/render state, but snapshotted beside it.
    combiner: CombinerState,
    /// Constant blender inputs. `blend_color.a` is mirrored into `other_mode`
    /// for alpha compare; the full RGBA values feed the framebuffer blender.
    blend_color: [u8; 4],
    fog_color: [u8; 4],
    /// Raw 32-bit RDP fill-color register. RGBA16 targets consume alternating
    /// high/low halfwords; RGBA32 targets consume the whole word per pixel.
    fill_color: u32,
    /// Most recent `G_RDPHALF_1` payload. F3DEX2's two-command BranchLessZ
    /// sequence stages its segmented target here before `G_BRANCH_Z`.
    rdp_half_1: Option<u32>,
    dl_depth: u32,
    /// Total commands decoded this frame (all streams), checked against
    /// [`MAX_DL_COMMANDS`] so a cyclic branch list terminates.
    cmds_decoded: u32,
    /// Texture-mapping decode state (SETTIMG image latch, tile descriptors,
    /// TLUT palette and G_TEXTURE enable/scale). See [`TexState`].
    tex: TexState,
    /// Vertex-lighting decode state (`G_MV_LIGHT` diffuse/ambient structs +
    /// `G_MW_NUMLIGHT` count). Applied at `G_VTX` time when the geometry
    /// mode's `G_LIGHTING` bit is set. See [`LightState`].
    lights: LightState,
    /// Screen-space X/Y directions loaded by `gSPLookAt`, consumed when
    /// `G_TEXTURE_GEN` replaces explicit vertex texture coordinates.
    look_at: LookAtState,
    fog: FogFactor,
    /// Explicit `gSPPerspNormalize` value. `None` means the display list has
    /// not programmed it; F3DEX2 ucode reloads preserve the live value.
    persp_normalize: PerspectiveNormalize,
    /// RSP primitive clipping rectangle relative to the viewport. This is
    /// deliberately separate from `G_CULLDL`'s ordinary frustum codes.
    clip_ratio: ClipRatio,
    /// First self-loaded text image not admitted as F3DEX2-compatible.
    /// Ordered decode stops at that load boundary.
    unsupported_ucode_reload: Option<UcodeDigest>,
}

/// Public F3DEX2 vertex-fog state. With `G_FOG` enabled, the RSP generates
/// shade alpha as `clamp(ndc_z * multiplier + offset, 0, 255)`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct FogFactor {
    multiplier: i16,
    offset: i16,
}

/// Limited-precision RSP perspective-divide normalization. In the float
/// reference path every nonzero scale cancels between transformed coordinates
/// and W; an explicitly programmed zero makes the divide degenerate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct PerspectiveNormalize(Option<u16>);

impl PerspectiveNormalize {
    fn rejects_geometry(self) -> bool {
        self.0 == Some(0)
    }
}

/// Per-side public `FRUSTRATIO_1..6` coefficients. The macro normally writes
/// the same ratio to all four fields, but the RSP state is updated one word at
/// a time, so retaining each side independently preserves command ordering.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ClipRatio {
    neg_x: u8,
    neg_y: u8,
    pos_x: u8,
    pos_y: u8,
}

impl Default for ClipRatio {
    fn default() -> Self {
        Self {
            neg_x: 1,
            neg_y: 1,
            pos_x: 1,
            pos_y: 1,
        }
    }
}

impl ClipRatio {
    fn write(&mut self, offset: u16, value: u32) {
        assert_eq!(
            value & 0xffff_0000,
            0,
            "G_MOVEWORD G_MW_CLIP ratio value must occupy the public low halfword"
        );
        let low = value as u16;
        match offset {
            G_MWO_CLIP_RNX | G_MWO_CLIP_RNY => {
                let ratio = u8::try_from(low).unwrap_or_else(|_| {
                    panic!(
                        "G_MOVEWORD G_MW_CLIP negative-side ratio {low:#06x} is not FRUSTRATIO_1..6"
                    )
                });
                assert!(
                    (1..=6).contains(&ratio),
                    "G_MOVEWORD G_MW_CLIP negative-side ratio {low:#06x} is not FRUSTRATIO_1..6"
                );
                if offset == G_MWO_CLIP_RNX {
                    self.neg_x = ratio;
                } else {
                    self.neg_y = ratio;
                }
            }
            G_MWO_CLIP_RPX | G_MWO_CLIP_RPY => {
                let signed = low as i16;
                assert!(
                    (-6..=-1).contains(&signed),
                    "G_MOVEWORD G_MW_CLIP positive-side ratio {low:#06x} is not -FRUSTRATIO_1..6"
                );
                let ratio = (-signed) as u8;
                if offset == G_MWO_CLIP_RPX {
                    self.pos_x = ratio;
                } else {
                    self.pos_y = ratio;
                }
            }
            _ => panic!(
                "G_MOVEWORD G_MW_CLIP offset {offset:#06x} is not a public clip-ratio destination"
            ),
        }
    }
}

fn fresh_decode_state() -> DecodeState {
    DecodeState {
        vtx_cache: [Vertex::default(); 64],
        ops: Vec::new(),
        segments: [0u32; 16],
        mvp: None,
        pending_forced_mvp: None,
        proj: None,
        modelview: identity(),
        mv_stack: Vec::new(),
        viewport: None,
        scissor: None,
        geometry_mode: 0,
        other_mode: OtherMode::default(),
        combiner: CombinerState::default(),
        blend_color: [0; 4],
        fog_color: [0; 4],
        fill_color: 0,
        rdp_half_1: None,
        dl_depth: 0,
        cmds_decoded: 0,
        tex: TexState::default(),
        lights: LightState::default(),
        look_at: LookAtState::default(),
        fog: FogFactor::default(),
        persp_normalize: PerspectiveNormalize::default(),
        clip_ratio: ClipRatio::default(),
        unsupported_ucode_reload: None,
    }
}

/// Reset only the state that public F3DEX2 self-loading does not maintain.
///
/// The public F3DEX2 release notes explicitly retain the display-list stack,
/// matrix stack, modelview/projection matrices, segment table, scissor,
/// other mode, perspective normalization, and viewport. They explicitly say
/// that the combined MP matrix, geometry mode, lights, and vertex cache are
/// not retained. Independent RDP state also remains live. State absent from
/// the exhaustive maintained list is reset rather than guessed persistent.
fn reset_rsp_state_from_ucode_load(state: &mut DecodeState) {
    state.vtx_cache = [Vertex::default(); 64];
    state.mvp = None;
    state.pending_forced_mvp = None;
    state.geometry_mode = 0;
    state.rdp_half_1 = None;
    state.tex.tex_enabled = false;
    state.tex.tex_tile = 0;
    state.tex.tex_max_level = 0;
    state.tex.tex_scale_s = 0.0;
    state.tex.tex_scale_t = 0.0;
    state.lights = LightState::default();
    state.look_at = LookAtState::default();
    state.fog = FogFactor::default();
    state.clip_ratio = ClipRatio::default();
}

/// The public F3DEX loadable-microcode contract initializes all RSP geometry
/// state, including segments, matrices, viewport, and display-list links.
/// RDP registers/TMEM remain independent and are deliberately not touched.
fn reset_legacy_rsp_state_from_ucode_load(state: &mut DecodeState) {
    assert_eq!(
        state.dl_depth, 0,
        "F3DEX/L3DEX G_LOAD_UCODE inside a called display list resets link state and cannot return"
    );
    state.vtx_cache = [Vertex::default(); 64];
    state.segments = [0; 16];
    state.mvp = None;
    state.pending_forced_mvp = None;
    state.proj = None;
    state.modelview = identity();
    state.mv_stack.clear();
    state.viewport = None;
    state.geometry_mode = 0;
    state.rdp_half_1 = None;
    state.tex.tex_enabled = false;
    state.tex.tex_tile = 0;
    state.tex.tex_max_level = 0;
    state.tex.tex_scale_s = 0.0;
    state.tex.tex_scale_t = 0.0;
    state.lights = LightState::default();
    state.look_at = LookAtState::default();
    state.fog = FogFactor::default();
    state.persp_normalize = PerspectiveNormalize::default();
    state.clip_ratio = ClipRatio::default();
}

fn initialize_geometry_family_state(state: &mut DecodeState, family: GeometryWireFamily) {
    match family {
        GeometryWireFamily::F3dlx => {
            // The public F3DLX contract starts with clipping enabled and
            // permits later G_CLIPPING set/clear commands.
            state.geometry_mode |= LEGACY_G_CLIPPING;
        }
        GeometryWireFamily::F3dlxRej
        | GeometryWireFamily::F3dex2Rej
        | GeometryWireFamily::F3dlx2Rej => {
            // The public reject-box contract starts at FRUSTRATIO_2.
            state.clip_ratio = ClipRatio {
                neg_x: 2,
                neg_y: 2,
                pos_x: 2,
                pos_y: 2,
            };
        }
        GeometryWireFamily::F3dex2 | GeometryWireFamily::F3dex2NoN | GeometryWireFamily::L3dex2 => {
            // F3DEX2 changed the public CLIPRATIO default from 1 to 2.
            state.clip_ratio = ClipRatio {
                neg_x: 2,
                neg_y: 2,
                pos_x: 2,
                pos_y: 2,
            };
        }
        GeometryWireFamily::Fast3d | GeometryWireFamily::F3dex | GeometryWireFamily::L3dex => {}
        GeometryWireFamily::F3dzex2 => {
            panic!(
                "F3DZEX2 state initialization requires an allowed-source execution specification"
            )
        }
    }
}

/// F3DEX2 vertex-lighting decode state (`F3DEX2-CONCEPTS.md` §2.4). The
/// RSP holds up to 7 directional lights plus one ambient; `num_dir` selects
/// how many directional slots are active, and the ambient light is the slot
/// at index `num_dir`. Directions are stored NORMALIZED in eye/model space
/// (s8 ÷127); the light-space transform uses the current modelview.
#[derive(Clone, Debug)]
struct LightState {
    /// Diffuse light slots (`G_MV_LIGHT`): direction (unit, s8÷127) + RGB
    /// color (0..1). Slot `num_dir` doubles as the ambient's color carrier
    /// when written, but ambient is read via `ambient` below.
    dir: [DirLight; MAX_LIGHTS],
    /// Ambient light color (0..1) -- the highest-numbered light slot.
    ambient: [f32; 3],
    /// Number of active directional lights (`G_MW_NUMLIGHT` / 24).
    num_dir: usize,
}

impl Default for LightState {
    fn default() -> Self {
        LightState {
            dir: [DirLight::default(); MAX_LIGHTS],
            // A conservative default: no ambient, no directionals, so a DL
            // that enables G_LIGHTING but (somehow) loaded no lights renders
            // dark rather than garbage -- but real OoT always loads both.
            ambient: [0.0, 0.0, 0.0],
            num_dir: 0,
        }
    }
}

/// One decoded directional light: a unit direction (light-space, s8÷127) and
/// an RGB diffuse color (0..1).
#[derive(Copy, Clone, Debug, Default)]
struct DirLight {
    dir: [f32; 3],
    col: [f32; 3],
}

const TMEM_BYTES: usize = 4 * 1024;
const TMEM_HALF_BYTES: usize = TMEM_BYTES / 2;

/// Physical RDP texture memory in bank order. A validity mask is retained per
/// byte so an uninitialized fetch traps by exact TMEM address instead of
/// manufacturing a color. Four-bit writes mark only the nibble transferred.
#[derive(Clone, PartialEq, Eq)]
struct Tmem {
    bytes: Box<[u8; TMEM_BYTES]>,
    valid: Box<[u8; TMEM_BYTES]>,
}

impl Default for Tmem {
    fn default() -> Self {
        Self {
            bytes: Box::new([0; TMEM_BYTES]),
            valid: Box::new([0; TMEM_BYTES]),
        }
    }
}

impl std::fmt::Debug for Tmem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let initialized_bits = self
            .valid
            .iter()
            .map(|mask| mask.count_ones() as usize)
            .sum::<usize>();
        f.debug_struct("Tmem")
            .field("initialized_bits", &initialized_bits)
            .finish_non_exhaustive()
    }
}

/// Immutable TMEM view captured with a primitive. The RDP register file may
/// be mutated by later commands before the backend rasterizes the operation,
/// so retaining only a tile number would violate command ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TmemTexture {
    storage: std::rc::Rc<Tmem>,
    tile: Tile,
    texture_lut: u8,
}

impl Tmem {
    #[inline]
    fn physical_byte(logical: usize, odd_row: bool) -> usize {
        // Programming Manual 13.9 and SGI Load Block usage notes: odd rows
        // exchange the two 32-bit longs in each 64-bit word. TMEM addressing
        // wraps in the 12-bit physical byte domain.
        (logical & (TMEM_BYTES - 1)) ^ if odd_row { 4 } else { 0 }
    }

    fn is_initialized(&self) -> bool {
        self.valid.iter().any(|mask| *mask != 0)
    }

    fn write_byte(&mut self, logical: usize, odd_row: bool, value: u8) {
        let address = Self::physical_byte(logical, odd_row);
        self.bytes[address] = value;
        self.valid[address] = u8::MAX;
    }

    fn write_nibble(&mut self, logical: usize, odd_row: bool, high: bool, value: u8) {
        let address = Self::physical_byte(logical, odd_row);
        let mask = if high { 0xf0 } else { 0x0f };
        let shifted = if high { value << 4 } else { value };
        self.bytes[address] = (self.bytes[address] & !mask) | (shifted & mask);
        self.valid[address] |= mask;
    }

    fn read_byte(&self, logical: usize, odd_row: bool, mask: u8, context: &str) -> u8 {
        let address = Self::physical_byte(logical, odd_row);
        assert_eq!(
            self.valid[address] & mask,
            mask,
            "{context} reads uninitialized TMEM bits at byte {address:#05x}"
        );
        self.bytes[address]
    }

    #[inline]
    fn row_base(tile: Tile, row: usize) -> usize {
        usize::from(tile.tmem) * 8 + row * usize::from(tile.line) * 8
    }

    fn write_texel(
        &mut self,
        tile: Tile,
        x: usize,
        row: usize,
        odd_row: bool,
        size: u8,
        value: u32,
    ) {
        let base = Self::row_base(tile, row);
        match size {
            G_IM_SIZ_4B => {
                self.write_nibble(base + x / 2, odd_row, x.is_multiple_of(2), value as u8)
            }
            G_IM_SIZ_8B => self.write_byte(base + x, odd_row, value as u8),
            G_IM_SIZ_16B => {
                for (byte, value) in (value as u16).to_be_bytes().into_iter().enumerate() {
                    self.write_byte(base + x * 2 + byte, odd_row, value);
                }
            }
            G_IM_SIZ_32B => {
                assert!(
                    base < TMEM_HALF_BYTES,
                    "32-bit texture load base {base:#05x} is outside low TMEM"
                );
                let [r, g, b, a] = value.to_be_bytes();
                let low = (base + x * 2) & (TMEM_HALF_BYTES - 1);
                self.write_byte(low, odd_row, r);
                self.write_byte(low + 1, odd_row, g);
                self.write_byte(low + TMEM_HALF_BYTES, odd_row, b);
                self.write_byte(low + TMEM_HALF_BYTES + 1, odd_row, a);
            }
            _ => unreachable!("RDP image size is a two-bit field"),
        }
    }

    fn write_yuv_pair(
        &mut self,
        tile: Tile,
        pair: usize,
        row: usize,
        odd_row: bool,
        yuyv: [u8; 4],
    ) {
        let base = Self::row_base(tile, row);
        assert!(
            base < TMEM_HALF_BYTES,
            "YUV texture load base {base:#05x} is outside low TMEM"
        );
        let low = (base + pair * 2) & (TMEM_HALF_BYTES - 1);
        let [y0, u, y1, v] = yuyv;
        self.write_byte(low, odd_row, u);
        self.write_byte(low + 1, odd_row, v);
        self.write_byte(low + TMEM_HALF_BYTES, odd_row, y0);
        self.write_byte(low + TMEM_HALF_BYTES + 1, odd_row, y1);
    }

    fn read_texel(&self, tile: Tile, x: usize, row: usize, size: u8) -> u32 {
        let base = Self::row_base(tile, row);
        let odd_row = (usize::from(tile.ult) / 4 + row) & 1 != 0;
        let context = format!("tile at TMEM word {} texel ({x}, {row})", tile.tmem);
        match size {
            G_IM_SIZ_4B => {
                let high = x.is_multiple_of(2);
                let mask = if high { 0xf0 } else { 0x0f };
                let byte = self.read_byte(base + x / 2, odd_row, mask, &context);
                u32::from(if high { byte >> 4 } else { byte & 0x0f })
            }
            G_IM_SIZ_8B => u32::from(self.read_byte(base + x, odd_row, 0xff, &context)),
            G_IM_SIZ_16B => {
                let bytes = [
                    self.read_byte(base + x * 2, odd_row, 0xff, &context),
                    self.read_byte(base + x * 2 + 1, odd_row, 0xff, &context),
                ];
                u32::from(u16::from_be_bytes(bytes))
            }
            G_IM_SIZ_32B => {
                assert!(
                    base < TMEM_HALF_BYTES,
                    "32-bit texture sample base {base:#05x} is outside low TMEM"
                );
                let low = (base + x * 2) & (TMEM_HALF_BYTES - 1);
                u32::from_be_bytes([
                    self.read_byte(low, odd_row, 0xff, &context),
                    self.read_byte(low + 1, odd_row, 0xff, &context),
                    self.read_byte(low + TMEM_HALF_BYTES, odd_row, 0xff, &context),
                    self.read_byte(low + TMEM_HALF_BYTES + 1, odd_row, 0xff, &context),
                ])
            }
            _ => unreachable!("RDP image size is a two-bit field"),
        }
    }

    fn write_tlut(&mut self, base_word: u16, index: usize, value: u16) {
        assert!(
            base_word >= 256,
            "G_LOADTLUT destination word {base_word} is outside high TMEM"
        );
        let base = usize::from(base_word) * 8 + index * 8;
        let [hi, lo] = value.to_be_bytes();
        for bank in 0..4 {
            self.write_byte(base + bank * 2, false, hi);
            self.write_byte(base + bank * 2 + 1, false, lo);
        }
    }

    fn read_tlut(&self, index: usize, mode: u8) -> [u8; 4] {
        assert!(
            index < 256,
            "CI texel index {index} exceeds the 256-entry TLUT"
        );
        let base = TMEM_HALF_BYTES + index * 8;
        let context = format!("TLUT index {index}");
        let value = u16::from_be_bytes([
            self.read_byte(base, false, 0xff, &context),
            self.read_byte(base + 1, false, 0xff, &context),
        ]);
        match mode {
            2 => rgba5551_to_rgba8888(value),
            3 => ia16_to_rgba8888((value >> 8) as u8, value as u8),
            _ => crate::render_unsupported_panic(
                "render.gbi.texture-lut-mode",
                format!("CI texture sampled with texture-LUT mode {mode}, expected RGBA16 or IA16"),
            ),
        }
    }
}

impl TmemTexture {
    fn sample(&self, x: usize, y: usize) -> [u8; 4] {
        if self.tile.fmt == G_IM_FMT_YUV && self.tile.siz == G_IM_SIZ_16B {
            let base = Tmem::row_base(self.tile, y);
            let odd_row = (usize::from(self.tile.ult) / 4 + y) & 1 != 0;
            let pair = x / 2;
            let context = format!("YUV tile at TMEM word {} texel ({x}, {y})", self.tile.tmem);
            let low = base + pair * 2;
            let high = low + TMEM_HALF_BYTES;
            let u = self.storage.read_byte(low, odd_row, 0xff, &context);
            let v = self.storage.read_byte(low + 1, odd_row, 0xff, &context);
            let luma = self
                .storage
                .read_byte(high + (x & 1), odd_row, 0xff, &context);
            return [luma, u, v, 255];
        }

        let raw = self.storage.read_texel(self.tile, x, y, self.tile.siz);
        match (self.tile.fmt, self.tile.siz) {
            (G_IM_FMT_RGBA, G_IM_SIZ_16B) => rgba5551_to_rgba8888(raw as u16),
            (G_IM_FMT_RGBA, G_IM_SIZ_32B) => raw.to_be_bytes(),
            (G_IM_FMT_RGBA, G_IM_SIZ_8B) | (G_IM_FMT_I, G_IM_SIZ_8B) => i8_to_rgba8888(raw as u8),
            (G_IM_FMT_RGBA, G_IM_SIZ_4B) | (G_IM_FMT_I, G_IM_SIZ_4B) => i4_to_rgba8888(raw as u8),
            (G_IM_FMT_IA, G_IM_SIZ_16B) => ia16_to_rgba8888((raw >> 8) as u8, raw as u8),
            (G_IM_FMT_IA, G_IM_SIZ_8B) => ia8_to_rgba8888(raw as u8),
            (G_IM_FMT_IA, G_IM_SIZ_4B) => ia4_to_rgba8888(raw as u8),
            (G_IM_FMT_CI, G_IM_SIZ_8B) if self.texture_lut == 0 => i8_to_rgba8888(raw as u8),
            (G_IM_FMT_CI, G_IM_SIZ_8B) => self.storage.read_tlut(raw as usize, self.texture_lut),
            (G_IM_FMT_CI, G_IM_SIZ_4B) => {
                let index = (usize::from(self.tile.palette) << 4) | raw as usize;
                self.storage.read_tlut(index, self.texture_lut)
            }
            (format, size) => crate::render_unsupported_panic(
                "render.gbi.texture-format",
                format!(
                    "TMEM tile uses unsupported texture format {format} size {size} at word {}",
                    self.tile.tmem
                ),
            ),
        }
    }
}

/// Texture-pipeline decode state (`F3DEX2-CONCEPTS.md` §5). Kept as a
/// sub-struct so the transform/geometry state above stays readable.
#[derive(Clone, Debug, Default)]
struct TexState {
    /// `G_SETTIMG`: the source texture image -- segmented addr + format +
    /// size-code. Latched; no data moves until a `G_LOAD*`.
    timg_addr: u32,
    timg_fmt: u8,
    timg_siz: u8,
    timg_width: u16,
    /// The 8 RDP tile descriptors (`G_SETTILE`/`G_SETTILESIZE`).
    tiles: [Tile; 8],
    /// The RDP's physical 4 KiB texture memory. Loads mutate this storage;
    /// render tiles merely reinterpret it.
    tmem: std::rc::Rc<Tmem>,
    /// `G_LOADTLUT` palette: up to 256 RGBA8888 entries decoded from the
    /// TLUT image (CI textures index into this).
    tlut: Vec<[u8; 4]>,
    /// `G_TEXTURE`: texturing enabled?
    tex_enabled: bool,
    /// `G_TEXTURE`: which tile descriptor is active (0-7).
    tex_tile: u8,
    /// `G_TEXTURE`: number of MIP levels following the primitive tile.
    tex_max_level: u8,
    /// `G_TEXTURE` S/T scale (U0.16 -> f32), applied to the raw vertex S/T
    /// before texel addressing.
    tex_scale_s: f32,
    tex_scale_t: f32,
}

/// RDP register/TMEM state that survives RSP task boundaries.
///
/// `G_TEXTURE` enable/tile/scale fields live in the RSP microcode state and
/// are deliberately cleared when capturing this snapshot. The texture-image
/// latch, tile descriptors, TMEM validity/data, TLUT, other mode, combiner,
/// constant colors, fill color, and scissor are RDP state and remain live for
/// the next HLE task or raw DPC submission.
#[derive(Clone, Debug, Default)]
pub(crate) struct RdpDecodeState {
    tex: TexState,
    scissor: Option<ScissorRect>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blend_color: [u8; 4],
    fog_color: [u8; 4],
    fill_color: u32,
}

impl RdpDecodeState {
    pub(crate) fn texture_filter(&self) -> TextureFilter {
        self.other_mode.texture_filter()
    }

    fn begin_task(&self) -> DecodeState {
        let mut state = fresh_decode_state();
        state.tex = self.tex.clone();
        state.scissor = self.scissor;
        state.other_mode = self.other_mode;
        state.combiner = self.combiner;
        state.blend_color = self.blend_color;
        state.fog_color = self.fog_color;
        state.fill_color = self.fill_color;
        state
    }

    fn commit_task(&mut self, state: &DecodeState) {
        self.tex = state.tex.clone();
        // These fields are owned by F3DEX2, not the RDP. rspboot/ucode
        // initialization establishes them for each task; carrying them here
        // would make a later task textured without issuing G_TEXTURE.
        self.tex.tex_enabled = false;
        self.tex.tex_tile = 0;
        self.tex.tex_max_level = 0;
        self.tex.tex_scale_s = 0.0;
        self.tex.tex_scale_t = 0.0;
        self.scissor = state.scissor;
        self.other_mode = state.other_mode;
        self.combiner = state.combiner;
        self.blend_color = state.blend_color;
        self.fog_color = state.fog_color;
        self.fill_color = state.fill_color;
    }

    /// Lower the public non-rotating S2DEX object rectangle to the same typed
    /// RDP operation produced by `G_TEXRECT`. Programming Manual Chapter 25,
    /// section 4.2.3 states that this is the operation S2DEX performs in the
    /// RSP. This initial slice deliberately admits only the mode-independent,
    /// non-flipped form; object render-mode corrections remain loud.
    pub(crate) fn object_rectangle(
        &mut self,
        sprite: crate::s2dex::ObjectSprite,
    ) -> Result<RenderOp, RenderError> {
        self.object_rectangle_with_mode(sprite, crate::s2dex::ObjectRenderMode::default())
    }

    /// Object rectangle lowering with the task-local S2DEX correction mode.
    /// The typed sampler already defines integer coordinates at texel centers,
    /// so `bilerp` records that the RSP's documented half-texel correction was
    /// requested and validates it against TF without applying a second shift.
    pub(crate) fn object_rectangle_with_mode(
        &mut self,
        sprite: crate::s2dex::ObjectSprite,
        object_mode: crate::s2dex::ObjectRenderMode,
    ) -> Result<RenderOp, RenderError> {
        let reject = |reason: String| RenderError::Backend {
            backend: "reference-s2dex",
            reason,
        };
        if sprite.padding_x != 0 || sprite.padding_y != 0 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE uObjSprite padding must be zero, got paddingX={} paddingY={}",
                sprite.padding_x, sprite.padding_y
            )));
        }
        if sprite.scale_w == 0 || sprite.scale_h == 0 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE scale must be nonzero, got scaleW={} scaleH={}",
                sprite.scale_w, sprite.scale_h
            )));
        }
        if sprite.scale_w > i16::MAX as u16 || sprite.scale_h > i16::MAX as u16 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE scale exceeds the RDP signed S5.10 gradient range: scaleW={} scaleH={}",
                sprite.scale_w, sprite.scale_h
            )));
        }
        if sprite.image_w == 0
            || sprite.image_h == 0
            || !sprite.image_w.is_multiple_of(32)
            || !sprite.image_h.is_multiple_of(32)
        {
            return Err(crate::render_unsupported_error(
                "reference-s2dex",
                "render.gbi.s2dex.object-rectangle",
                format!(
                    "G_OBJ_RECTANGLE initial slice requires positive whole-texel u10.5 dimensions, got imageW={} imageH={}",
                    sprite.image_w, sprite.image_h
                ),
            ));
        }
        if sprite.image_stride == 0 || sprite.image_stride > 0x01ff {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE imageStride={} is outside the RDP tile-line range 1..=511",
                sprite.image_stride
            )));
        }
        if sprite.image_address > 0x01ff {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE imageAdrs={} exceeds the RDP tile TMEM-address range 0..=511",
                sprite.image_address
            )));
        }
        if sprite.image_format > G_IM_FMT_I || sprite.image_size > G_IM_SIZ_32B {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE texture format={} size={} is outside public G_IM_FMT/G_IM_SIZ encodings",
                sprite.image_format, sprite.image_size
            )));
        }
        if sprite.image_palette > 7 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE imagePal={} is outside the public S2DEX range 0..=7",
                sprite.image_palette
            )));
        }
        if sprite.image_flags != 0 {
            return Err(crate::render_unsupported_error(
                "reference-s2dex",
                "render.gbi.s2dex.object-rectangle",
                format!(
                    "G_OBJ_RECTANGLE imageFlags={:#04x} requests unsupported S/T flip correction",
                    sprite.image_flags
                ),
            ));
        }

        let mut state = self.begin_task();
        let width = u32::from(sprite.image_w / 32);
        let height = u32::from(sprite.image_h / 32);
        let tile = &mut state.tex.tiles[0];
        tile.fmt = sprite.image_format;
        tile.siz = sprite.image_size;
        tile.line = sprite.image_stride;
        tile.tmem = sprite.image_address;
        tile.palette = sprite.image_palette;
        tile.clamp_s = true;
        tile.clamp_t = true;
        tile.mirror_s = false;
        tile.mirror_t = false;
        tile.mask_s = 0;
        tile.mask_t = 0;
        tile.shift_s = 0;
        tile.shift_t = 0;
        tile.uls = 0;
        tile.ult = 0;
        tile.lrs = u16::try_from((width - 1) * 4).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE image width {width} exceeds tile bounds"
            ))
        })?;
        tile.lrt = u16::try_from((height - 1) * 4).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE image height {height} exceeds tile bounds"
            ))
        })?;

        let ulx = f32::from(sprite.obj_x) / 4.0;
        let uly = f32::from(sprite.obj_y) / 4.0;
        let screen_width = sprite.image_w as f32 * 32.0 / sprite.scale_w as f32;
        let screen_height = sprite.image_h as f32 * 32.0 / sprite.scale_h as f32;
        let cycle_type = state.other_mode.cycle_type();
        if cycle_type == CycleType::Fill {
            return Err(crate::render_unsupported_error(
                "reference-s2dex",
                "render.gbi.s2dex.object-rectangle",
                "G_OBJ_RECTANGLE cannot execute in Fill cycle; S2DEX supports one-cycle, two-cycle, and copy modes",
            ));
        }
        if cycle_type == CycleType::Copy && sprite.scale_w != 1 << 10 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE copy mode cannot scale X; scaleW={} must be 1024",
                sprite.scale_w
            )));
        }
        if cycle_type != CycleType::Copy {
            use crate::s2dex::{ObjectFilterCorrection, ObjectTextureClamp};
            match (
                state.other_mode.texture_filter(),
                object_mode.filter_correction,
            ) {
                (TextureFilter::Point, ObjectFilterCorrection::PointOrAverage)
                | (TextureFilter::Bilinear, ObjectFilterCorrection::Bilinear) => {}
                (TextureFilter::Average, ObjectFilterCorrection::PointOrAverage)
                    if object_mode.perimeter.is_none()
                        && object_mode.texture_clamp == ObjectTextureClamp::Perimeter => {}
                (TextureFilter::Point, ObjectFilterCorrection::Bilinear) => {
                    return Err(reject(
                        "G_OBJ_RECTANGLE G_OBJRM_BILERP is set while the RDP texture filter is Point"
                            .into(),
                    ));
                }
                (TextureFilter::Bilinear, ObjectFilterCorrection::PointOrAverage) => {
                    return Err(reject(
                        "G_OBJ_RECTANGLE Bilinear texture filter requires G_OBJRM_BILERP correction"
                            .into(),
                    ));
                }
                (TextureFilter::Average, ObjectFilterCorrection::Bilinear) => {
                    return Err(reject(
                        "G_OBJ_RECTANGLE Average texture filter does not use G_OBJRM_BILERP correction"
                            .into(),
                    ));
                }
                (TextureFilter::Average, ObjectFilterCorrection::PointOrAverage) => {
                    return Err(crate::render_unsupported_error(
                        "reference-s2dex",
                        "render.gbi.s2dex.object-rectangle",
                        "G_OBJ_RECTANGLE Average texture filter combined with perimeter correction or G_OBJRM_NOTXCLAMP requires unpublished filter-footprint arithmetic",
                    ));
                }
                (filter, _) => {
                    return Err(crate::render_unsupported_error(
                        "reference-s2dex",
                        "render.gbi.s2dex.object-rectangle",
                        format!(
                            "G_OBJ_RECTANGLE texture filter {filter:?} has no admitted S2DEX correction mode"
                        ),
                    ));
                }
            }
        } else if object_mode.filter_correction == crate::s2dex::ObjectFilterCorrection::Bilinear {
            return Err(reject(
                "G_OBJ_RECTANGLE Copy cycle does not support G_OBJRM_BILERP".into(),
            ));
        }
        let inclusive = cycle_type == CycleType::Copy;
        let storage = state.tex.tmem.clone();
        let texture_lut = state.other_mode.texture_lut();
        let rectangle = TextureRectangle {
            ulx,
            uly,
            lrx: ulx + screen_width - if inclusive { 1.0 } else { 0.0 },
            lry: uly + screen_height - if inclusive { 1.0 } else { 0.0 },
            tile: 0,
            s: 0.0,
            t: 0.0,
            dsdx: if inclusive {
                4 << 10
            } else {
                sprite.scale_w as i16
            },
            dtdy: sprite.scale_h as i16,
            flip: false,
            other_mode: state.other_mode,
            combiner: state.combiner,
            blender: active_blender(&state),
            scissor: state.scissor,
            texture: texture_for_tile(&state.tex, 0, texture_lut, &storage),
            texture1: texture_for_tile(&state.tex, 1, texture_lut, &storage),
        };
        self.commit_task(&state);
        Ok(RenderOp::TextureRectangle(rectangle))
    }
}

/// One RDP tile descriptor (`G_SETTILE` + `G_SETTILESIZE`,
/// `F3DEX2-CONCEPTS.md` §5.1) -- only the fields the reference sampler needs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Tile {
    fmt: u8,
    siz: u8,
    /// Line stride in 64-bit words (`G_SETTILE` `line`).
    line: u16,
    /// Base address in 64-bit TMEM words (`G_SETTILE` `tmem`).
    tmem: u16,
    /// TLUT palette bank (CI4 uses this as the high nibble of the index).
    palette: u8,
    clamp_s: bool,
    clamp_t: bool,
    mirror_s: bool,
    mirror_t: bool,
    mask_s: u8,
    mask_t: u8,
    shift_s: u8,
    shift_t: u8,
    /// Tile active extent from `G_SETTILESIZE` (10.2 -> ÷4 texels).
    uls: u16,
    ult: u16,
    lrs: u16,
    lrt: u16,
}

/// Apply the public `G_SETTILE` wire fields without disturbing the extent
/// owned by `G_SETTILESIZE`/load commands.
fn apply_set_tile(tile: &mut Tile, w0: u32, w1: u32) {
    tile.fmt = ((w0 >> 21) & 0x07) as u8;
    tile.siz = ((w0 >> 19) & 0x03) as u8;
    tile.line = ((w0 >> 9) & 0x01ff) as u16;
    tile.tmem = (w0 & 0x01ff) as u16;
    tile.palette = ((w1 >> 20) & 0x0f) as u8;
    let cm_t = ((w1 >> 18) & 0x03) as u8;
    tile.mask_t = ((w1 >> 14) & 0x0f) as u8;
    tile.shift_t = ((w1 >> 10) & 0x0f) as u8;
    let cm_s = ((w1 >> 8) & 0x03) as u8;
    tile.mask_s = ((w1 >> 4) & 0x0f) as u8;
    tile.shift_s = (w1 & 0x0f) as u8;
    tile.clamp_s = cm_s & 0x02 != 0;
    tile.clamp_t = cm_t & 0x02 != 0;
    tile.mirror_s = cm_s & 0x01 != 0;
    tile.mirror_t = cm_t & 0x01 != 0;
}

/// Parsed viewport: screen scale/translate in pixels (x, y) plus a depth
/// scale/translate (z), all already ÷4 from the N64 quarter-pixel encoding
/// (`F3DEX2-CONCEPTS.md` §3.5).
#[derive(Copy, Clone, Debug)]
struct Viewport {
    sx: f32,
    sy: f32,
    sz: f32,
    tx: f32,
    ty: f32,
    tz: f32,
}

/// Max `G_DL` *call* (G_DL_PUSH) recursion depth honored, matching the real
/// F3DEX2 display-list return stack (18 entries; the older 10-entry figure
/// is F3D/F3DEX). Only pushes count -- a gsSPBranchList tail-jump replaces
/// the DL pointer and consumes NO stack entry on hardware, so branch chains
/// (which OoT uses liberally) must not count against this.
const MAX_DL_DEPTH: u32 = 18;

/// Whole-decode command budget: bounds a cyclic/corrupt DL (e.g. a branch
/// list that branches to itself), which the hardware would spin on forever.
/// A real OoT frame decodes on the order of 10^4 commands; 2^20 is far above
/// any legitimate frame while still terminating promptly on a cycle.
const MAX_DL_COMMANDS: u32 = 1 << 20;

/// The simple ("reference-fixture") F3D-style decoder retained for backward
/// compatibility: `G_VTX`/`G_TRI1`/`G_TRI2`/`G_ENDDL` with raw screen-space
/// `ob` coords, non-segmented addresses in `w1`, and the pre-existing
/// vertex/index packing (`n<<12 | v0`; indices `(v0<<16)|(v1<<8)|v2` as
/// plain cache slots). This is what the original hand-built fixtures and the
/// `fn64-abi` executor-seam test plant, so it MUST stay bit-compatible with
/// them. Real OoT display lists use [`decode_display_list_f3dex2`] instead.
pub fn decode_display_list(rdram: &[u8], dl_addr: u32) -> Result<Vec<Triangle>, RenderError> {
    let mut vtx_cache = [Vertex::default(); 32];
    let mut tris = Vec::new();
    let mut pc = dl_addr as usize;

    loop {
        if pc + 8 > rdram.len() {
            return Err(RenderError::Backend {
                backend: "reference-simple",
                reason: format!(
                    "display list reached truncated command at {pc:#010x} without G_ENDDL"
                ),
            });
        }
        let command_pc = pc;
        let w0 = u32::from_be_bytes(rdram[pc..pc + 4].try_into().unwrap());
        let w1 = u32::from_be_bytes(rdram[pc + 4..pc + 8].try_into().unwrap());
        let opcode = (w0 >> 24) as u8;
        pc += 8;

        match opcode {
            G_VTX => {
                // Original packing: w0 low 20 bits = n<<12 | v0; w1 = vtx
                // array address (non-segmented). Raw ob x/y are screen
                // coords -- no transform.
                let n = ((w0 >> 12) & 0xFF) as usize;
                let v0 = (w0 & 0xFF) as usize;
                let addr = w1 as usize;
                let cache_end = v0.checked_add(n).ok_or_else(|| RenderError::Backend {
                    backend: "reference-simple",
                    reason: format!("G_VTX cache range overflows at {command_pc:#010x}"),
                })?;
                let byte_len = n
                    .checked_mul(VTX_STRIDE)
                    .ok_or_else(|| RenderError::Backend {
                        backend: "reference-simple",
                        reason: format!("G_VTX byte count overflows at {command_pc:#010x}"),
                    })?;
                let addr_end = addr
                    .checked_add(byte_len)
                    .ok_or_else(|| RenderError::Backend {
                        backend: "reference-simple",
                        reason: format!("G_VTX RDRAM range overflows at {command_pc:#010x}"),
                    })?;
                if n == 0 || cache_end > vtx_cache.len() || addr_end > rdram.len() {
                    return Err(RenderError::Backend {
                        backend: "reference-simple",
                        reason: format!(
                            "G_VTX at {command_pc:#010x} selects n={n}, cache={v0}..{cache_end}, RDRAM={addr:#010x}..{addr_end:#010x}"
                        ),
                    });
                }
                for i in 0..n {
                    let off = addr + i * VTX_STRIDE;
                    let x = i16::from_be_bytes([rdram[off], rdram[off + 1]]) as f32;
                    let y = i16::from_be_bytes([rdram[off + 2], rdram[off + 3]]) as f32;
                    let cn = &rdram[off + 12..off + 16];
                    vtx_cache[v0 + i] = Vertex {
                        x,
                        y,
                        z: 0.0, // simple reference path: coplanar, no depth
                        r: cn[0],
                        g: cn[1],
                        b: cn[2],
                        a: cn[3],
                        s: 0.0, // simple reference path: untextured
                        t: 0.0,
                        w: 1.0, // simple path: everything in front of camera
                        z_screen: 0,
                        clip_code: 0,
                        clip_position: None,
                    };
                }
            }
            G_TRI1 => {
                let idx = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
            }
            G_TRI2 => {
                let idx_a = [(w0 >> 16) & 0xFF, (w0 >> 8) & 0xFF, w0 & 0xFF];
                let idx_b = [(w1 >> 16) & 0xFF, (w1 >> 8) & 0xFF, w1 & 0xFF];
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx_a,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
                if let Some(t) = resolve_tri(
                    &vtx_cache,
                    idx_b,
                    CullMode::None,
                    None,
                    OtherMode::default(),
                    CombinerState::default(),
                    BlenderState::default(),
                ) {
                    tris.push(t);
                }
            }
            G_ENDDL => break,
            G_NOOP if w0 == 0 && w1 == 0 => {}
            _ => {
                return Err(crate::render_unsupported_error(
                    "reference-simple",
                    "render.gbi.simple.command",
                    format!(
                        "unsupported opcode {opcode:#04x} at {command_pc:#010x} (w0={w0:#010x}, w1={w1:#010x})"
                    ),
                ));
            }
        }
    }
    Ok(tris)
}

fn decode_display_list_f3dex2_state(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<DecodeState, RenderError> {
    let mut state = fresh_decode_state();
    let mut scratch = rdram.to_vec();
    let mut family = GeometryWireFamily::F3dex2;
    #[cfg(not(test))]
    projdump::reset_frame();
    decode_stream(&mut scratch, dl_addr, &mut state, None, None, &mut family);
    #[cfg(not(test))]
    projdump::summary();
    Ok(state)
}

fn execute_display_list_f3dex2_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
) -> Result<DecodeState, RenderError> {
    execute_display_list_f3dex2_state_with_catalog(rdram, rsp_memory, dl_addr, None)
}

fn execute_display_list_f3dex2_state_with_catalog(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: Option<&F3dex2UcodeCatalog>,
) -> Result<DecodeState, RenderError> {
    execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        catalog,
        None,
        GeometryWireFamily::F3dex2,
    )
}

fn execute_display_list_f3dex2_state_with_catalog_and_rdp(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: Option<&F3dex2UcodeCatalog>,
    rdp_state: Option<&mut RdpDecodeState>,
    initial_family: GeometryWireFamily,
) -> Result<DecodeState, RenderError> {
    let mut state = rdp_state
        .as_deref()
        .map(RdpDecodeState::begin_task)
        .unwrap_or_else(fresh_decode_state);
    #[cfg(not(test))]
    projdump::reset_frame();
    let mut family = initial_family;
    initialize_geometry_family_state(&mut state, family);
    decode_stream(
        rdram,
        dl_addr,
        &mut state,
        Some(rsp_memory),
        catalog,
        &mut family,
    );
    #[cfg(not(test))]
    projdump::summary();
    if let Some(digest) = state.unsupported_ucode_reload {
        return Err(RenderError::RequiresLle {
            ucode_sha256: digest.as_bytes(),
        });
    }
    if let Some(rdp_state) = rdp_state {
        rdp_state.commit_task(&state);
    }
    Ok(state)
}

/// Decode a bounded raw RDP stream. Unlike an RSP display list, opcodes
/// 0x08..0x0f are variable-width edge/coefficient commands.
pub fn decode_raw_rdp_ops(rdram: &[u8], start: u32) -> Result<Vec<RenderOp>, RenderError> {
    let mut state = fresh_decode_state();
    let mut scratch = rdram.to_vec();
    let mut family = GeometryWireFamily::F3dex2;
    decode_stream_impl(
        &mut scratch,
        start,
        &mut state,
        true,
        None,
        None,
        &mut family,
    );
    Ok(state.ops)
}

pub(crate) fn decode_raw_rdp_ops_with_state(
    rdram: &[u8],
    start: u32,
    rdp_state: &mut RdpDecodeState,
) -> Result<Vec<RenderOp>, RenderError> {
    let mut state = rdp_state.begin_task();
    let mut scratch = rdram.to_vec();
    let mut family = GeometryWireFamily::F3dex2;
    decode_stream_impl(
        &mut scratch,
        start,
        &mut state,
        true,
        None,
        None,
        &mut family,
    );
    rdp_state.commit_task(&state);
    Ok(state.ops)
}

/// Decode a real F3DEX2 display-list graph to ordered RSP/RDP operations.
/// State mutations, framebuffer targets, fills, syncs, and triangles retain
/// command order across nested display lists.
pub fn decode_display_list_f3dex2_ops(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(decode_display_list_f3dex2_state(rdram, dl_addr)?.ops)
}

/// Execute an F3DEX2 display-list graph against the console's live RDRAM and
/// persistent RSP memories. Unlike the read-only inspection helper above,
/// this path applies `G_DMA_IO` in command order, so a debug DMA write can
/// change bytes consumed by a later command in the same task.
pub fn execute_display_list_f3dex2_ops(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(execute_display_list_f3dex2_state(rdram, rsp_memory, dl_addr)?.ops)
}

/// Execute with an exact catalog for compatible self-loaded text images.
/// The backend separately verifies the initial live IMEM image against this
/// same catalog before entering this decoder; any changed, unlisted
/// generation stops at `G_LOAD_UCODE` before HLE can interpret its following
/// commands under the wrong microcode family.
pub fn execute_display_list_f3dex2_ops_admitted(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &F3dex2UcodeCatalog,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(
        execute_display_list_f3dex2_state_with_catalog(rdram, rsp_memory, dl_addr, Some(catalog))?
            .ops,
    )
}

#[cfg(test)]
pub(crate) fn execute_display_list_f3dex2_ops_admitted_with_rdp_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &F3dex2UcodeCatalog,
    rdp_state: &mut RdpDecodeState,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        Some(catalog),
        Some(rdp_state),
        GeometryWireFamily::F3dex2,
    )?
    .ops)
}

pub(crate) fn execute_display_list_geometry_ops_admitted_with_rdp_state(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    dl_addr: u32,
    catalog: &GeometryUcodeCatalog,
    rdp_state: &mut RdpDecodeState,
    family: GeometryWireFamily,
) -> Result<Vec<RenderOp>, RenderError> {
    Ok(execute_display_list_f3dex2_state_with_catalog_and_rdp(
        rdram,
        rsp_memory,
        dl_addr,
        Some(catalog),
        Some(rdp_state),
        family,
    )?
    .ops)
}

/// Validate a bounded DRAM-backed raw RDP command range before the reference
/// backend executes it. The HLE decoder currently implements the listed
/// rectangle/image/texture/state commands and all eight edge/coefficient
/// triangle layouts. Unmodeled RDP state opcodes remain loud instead of
/// disappearing through `skip_opcode`.
pub fn validate_raw_rdp_command_range(
    rdram: &[u8],
    start: u32,
    end: u32,
) -> Result<(), RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "reference",
        reason,
    };
    if start >= end || !start.is_multiple_of(8) || !end.is_multiple_of(8) {
        return Err(reject(format!(
            "raw RDP range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
        )));
    }
    if end as usize > rdram.len() {
        return Err(reject(format!(
            "raw RDP range end {end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        )));
    }
    let mut pc = start;
    while pc < end {
        let wire_opcode = (read_u32(rdram, pc as usize) >> 24) as u8;
        let triangle_opcode = wire_opcode & 0x3f;
        let opcode = if matches!(triangle_opcode, 0x08..=0x0f) {
            triangle_opcode
        } else {
            wire_opcode
        };
        let supported = matches!(
            opcode,
            G_NOOP | 0x08
                ..=0x0f
                    | G_TEXRECT
                    | G_TEXRECTFLIP
                    | G_RDPLOADSYNC
                    | G_RDPPIPESYNC
                    | G_RDPTILESYNC
                    | G_RDPFULLSYNC
                    | G_SETOTHERMODE_L
                    | G_SETOTHERMODE_H
                    | G_RDPSETOTHERMODE
                    | G_SETSCISSOR
                    | G_SETKEYGB
                    | G_SETKEYR
                    | G_SETCONVERT
                    | G_SETPRIMDEPTH
                    | G_LOADTLUT
                    | G_SETTILESIZE
                    | G_LOADBLOCK
                    | G_LOADTILE
                    | G_SETTILE
                    | G_FILLRECT
                    | G_SETFILLCOLOR
                    | G_SETFOGCOLOR
                    | G_SETBLENDCOLOR
                    | G_SETPRIMCOLOR
                    | G_SETENVCOLOR
                    | G_SETCOMBINE
                    | G_SETTIMG
                    | G_SETZIMG
                    | G_SETCIMG
        );
        if !supported {
            return Err(crate::render_unsupported_error(
                "reference",
                "render.gbi.raw-rdp.command",
                format!(
                    "raw RDP opcode {} ({opcode:#04x}) at {pc:#010x} is unsupported",
                    raw_rdp_opcode_name(opcode)
                ),
            ));
        }
        let width = raw_rdp_command_width(opcode).unwrap_or(8);
        pc = pc.checked_add(width).ok_or_else(|| {
            reject(format!(
                "raw RDP command at {pc:#010x} overflows address space"
            ))
        })?;
        if pc > end {
            return Err(reject(format!(
                "raw RDP {} at {:#010x} is truncated by range end {end:#010x}",
                raw_rdp_opcode_name(opcode),
                pc - width
            )));
        }
    }
    Ok(())
}

/// Inspect one exact raw DPC range for the command that generates the public
/// DP completion interrupt. Variable-width triangle payload words are skipped
/// structurally, so coefficient data cannot be mistaken for an opcode.
pub fn raw_rdp_full_sync_status(
    rdram: &[u8],
    start: u32,
    end: u32,
) -> Result<fn64_render::DpFullSyncStatus, RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "rdp-full-sync-inspection",
        reason,
    };
    if start >= end || !start.is_multiple_of(8) || !end.is_multiple_of(8) {
        return Err(reject(format!(
            "raw RDP range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
        )));
    }
    if end as usize > rdram.len() {
        return Err(reject(format!(
            "raw RDP range end {end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        )));
    }
    let mut reached = false;
    let mut pc = start;
    while pc < end {
        let wire_opcode = (read_u32(rdram, pc as usize) >> 24) as u8;
        let triangle_opcode = wire_opcode & 0x3f;
        let opcode = if matches!(triangle_opcode, 0x08..=0x0f) {
            triangle_opcode
        } else {
            wire_opcode
        };
        reached |= opcode == G_RDPFULLSYNC;
        let width = raw_rdp_command_width(opcode).ok_or_else(|| {
            reject(format!(
                "raw RDP opcode {opcode:#04x} at {pc:#010x} has no public command width"
            ))
        })?;
        pc = pc.checked_add(width).ok_or_else(|| {
            reject(format!(
                "raw RDP command at {pc:#010x} overflows address space"
            ))
        })?;
        if pc > end {
            return Err(reject(format!(
                "raw RDP command at {:#010x} is truncated by range end {end:#010x}",
                pc - width
            )));
        }
    }
    Ok(if reached {
        fn64_render::DpFullSyncStatus::Reached
    } else {
        fn64_render::DpFullSyncStatus::NotReached
    })
}

fn raw_rdp_opcode_name(opcode: u8) -> &'static str {
    match opcode {
        0x08 => "RDP_TRI_FILL",
        0x09 => "RDP_TRI_FILL_ZBUFF",
        0x0a => "RDP_TRI_TXTR",
        0x0b => "RDP_TRI_TXTR_ZBUFF",
        0x0c => "RDP_TRI_SHADE",
        0x0d => "RDP_TRI_SHADE_ZBUFF",
        0x0e => "RDP_TRI_SHADE_TXTR",
        0x0f => "RDP_TRI_SHADE_TXTR_ZBUFF",
        _ => opcode_name(opcode),
    }
}

/// Byte width of one raw RDP command. Triangle sizes concatenate the edge,
/// shade, texture, and Z groups from SGI *RDP Command Summary* Table 11.
fn raw_rdp_command_width(opcode: u8) -> Option<u32> {
    let triangle_opcode = opcode & 0x3f;
    if matches!(triangle_opcode, 0x08..=0x0f) {
        return Some(match triangle_opcode {
            0x08 => 32,
            0x09 => 48,
            0x0a => 96,
            0x0b => 112,
            0x0c => 96,
            0x0d => 112,
            0x0e => 160,
            0x0f => 176,
            _ => unreachable!(),
        });
    }
    Some(match opcode {
        G_NOOP => 8,
        G_TEXRECT | G_TEXRECTFLIP => 16,
        G_RDPLOADSYNC..=0xFF => 8,
        _ => return None,
    })
}

fn decode_rdp_edge_coefficients(rdram: &[u8], pc: usize) -> Option<RdpEdgeCoefficients> {
    if pc.checked_add(32)? > rdram.len() {
        return None;
    }
    let w0 = read_u32(rdram, pc);
    let w1 = read_u32(rdram, pc + 4);
    Some(RdpEdgeCoefficients {
        right_major: w0 & (1 << 23) != 0,
        level: ((w0 >> 19) & 0x07) as u8,
        tile: ((w0 >> 16) & 0x07) as u8,
        yl: sign_extend_u32(w0 & 0x3fff, 14) as i16,
        ym: sign_extend_u32((w1 >> 16) & 0x3fff, 14) as i16,
        yh: sign_extend_u32(w1 & 0x3fff, 14) as i16,
        xl: read_u32(rdram, pc + 8) as i32,
        dxldy: read_u32(rdram, pc + 12) as i32,
        xh: read_u32(rdram, pc + 16) as i32,
        dxhdy: read_u32(rdram, pc + 20) as i32,
        xm: read_u32(rdram, pc + 24) as i32,
        dxmdy: read_u32(rdram, pc + 28) as i32,
    })
}

fn decode_rdp_z_coefficients(rdram: &[u8], pc: usize) -> Option<RdpZCoefficients> {
    if pc.checked_add(16)? > rdram.len() {
        return None;
    }
    Some(RdpZCoefficients {
        z: read_u32(rdram, pc) as i32,
        dzdx: read_u32(rdram, pc + 4) as i32,
        dzde: read_u32(rdram, pc + 8) as i32,
        dzdy: read_u32(rdram, pc + 12) as i32,
    })
}

fn decode_rdp_shade_coefficients(rdram: &[u8], pc: usize) -> Option<RdpShadeCoefficients> {
    if pc.checked_add(64)? > rdram.len() {
        return None;
    }
    let components = |integer_offset: usize, fraction_offset: usize| {
        let integer_rg = read_u32(rdram, pc + integer_offset);
        let integer_ba = read_u32(rdram, pc + integer_offset + 4);
        let fraction_rg = read_u32(rdram, pc + fraction_offset);
        let fraction_ba = read_u32(rdram, pc + fraction_offset + 4);
        [
            fixed_16_16(integer_rg >> 16, fraction_rg >> 16),
            fixed_16_16(integer_rg, fraction_rg),
            fixed_16_16(integer_ba >> 16, fraction_ba >> 16),
            fixed_16_16(integer_ba, fraction_ba),
        ]
    };
    Some(RdpShadeCoefficients {
        color: components(0, 16),
        dcdx: components(8, 24),
        dcde: components(32, 48),
        dcdy: components(40, 56),
    })
}

fn decode_rdp_texture_coefficients(rdram: &[u8], pc: usize) -> Option<RdpTextureCoefficients> {
    if pc.checked_add(64)? > rdram.len() {
        return None;
    }
    let components = |integer_offset: usize, fraction_offset: usize| {
        let integer_st = read_u32(rdram, pc + integer_offset);
        let integer_w = read_u32(rdram, pc + integer_offset + 4);
        let fraction_st = read_u32(rdram, pc + fraction_offset);
        let fraction_w = read_u32(rdram, pc + fraction_offset + 4);
        [
            fixed_16_16(integer_st >> 16, fraction_st >> 16),
            fixed_16_16(integer_st, fraction_st),
            fixed_16_16(integer_w >> 16, fraction_w >> 16),
        ]
    };
    Some(RdpTextureCoefficients {
        stw: components(0, 16),
        dstdx: components(8, 24),
        dstde: components(32, 48),
        dstdy: components(40, 56),
    })
}

fn fixed_16_16(integer: u32, fraction: u32) -> i32 {
    (i32::from(integer as u16 as i16) << 16) | i32::from(fraction as u16)
}

fn sign_extend_u32(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

/// Compatibility view of F3DEX2 decode for callers that inspect geometry.
/// The reference backend consumes [`decode_display_list_f3dex2_ops`] so it
/// does not discard non-triangle RDP work.
pub fn decode_display_list_f3dex2(
    rdram: &[u8],
    dl_addr: u32,
) -> Result<Vec<Triangle>, RenderError> {
    Ok(decode_display_list_f3dex2_ops(rdram, dl_addr)?
        .into_iter()
        .filter_map(|op| match op {
            RenderOp::Triangle(triangle) => Some(triangle),
            _ => None,
        })
        .collect())
}

/// Produce a lossless command-word walk of an F3DEX2 display-list graph for
/// differential diagnostics. This follows the same public `G_DL` call versus
/// branch rules and `G_MOVEWORD/G_MW_SEGMENT` address updates as the decoder,
/// but does not interpret rendering state. Pointer-bearing commands include a
/// bounded content fingerprint at their resolved RDRAM target, so a trace can
/// distinguish a valid submitted graph from a dangling/empty DMA range without
/// copying game data into this repository. The caller owns where the returned
/// text is written; the RT64 task-dump hook writes only to an explicitly
/// requested untracked diagnostic directory.
pub(crate) fn trace_display_list_f3dex2(rdram: &[u8], dl_addr: u32) -> String {
    struct TraceState {
        segments: [u32; 16],
        commands: u32,
        opcodes: BTreeMap<u8, u32>,
        text: String,
    }

    fn fingerprint(rdram: &[u8], start: usize, requested_len: usize) -> String {
        if start >= rdram.len() {
            return format!("target={start:#08x} OUT_OF_BOUNDS");
        }
        let end = start.saturating_add(requested_len).min(rdram.len());
        let bytes = &rdram[start..end];
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let nonzero = bytes.iter().filter(|&&byte| byte != 0).count();
        format!(
            "target={start:#08x} bytes={} nonzero={} fnv1a64={hash:016x}",
            bytes.len(),
            nonzero
        )
    }

    fn trace_stream(rdram: &[u8], dl_addr: u32, depth: u32, state: &mut TraceState) {
        let mut pc = resolve_addr(&state.segments, dl_addr);
        writeln!(
            state.text,
            "ENTER depth={depth} segmented={dl_addr:#010x} resolved={pc:#08x}"
        )
        .expect("writing a display-list trace to String cannot fail");

        loop {
            if pc + 8 > rdram.len() {
                writeln!(state.text, "STOP depth={depth} pc={pc:#08x} OUT_OF_BOUNDS")
                    .expect("writing a display-list trace to String cannot fail");
                break;
            }
            state.commands += 1;
            if state.commands > MAX_DL_COMMANDS {
                writeln!(
                    state.text,
                    "STOP depth={depth} command_budget={MAX_DL_COMMANDS} exceeded"
                )
                .expect("writing a display-list trace to String cannot fail");
                break;
            }

            let command_pc = pc;
            let w0 = read_u32(rdram, pc);
            let w1 = read_u32(rdram, pc + 4);
            let opcode = (w0 >> 24) as u8;
            pc += 8;
            *state.opcodes.entry(opcode).or_default() += 1;

            let reference = match opcode {
                G_VTX => {
                    let n = ((w0 >> 12) & 0xFF) as usize;
                    Some(fingerprint(
                        rdram,
                        resolve_addr(&state.segments, w1),
                        n.saturating_mul(16),
                    ))
                }
                G_MTX => Some(fingerprint(rdram, resolve_addr(&state.segments, w1), 64)),
                G_MOVEMEM | G_SETTIMG | G_DL => {
                    Some(fingerprint(rdram, resolve_addr(&state.segments, w1), 64))
                }
                _ => None,
            };
            writeln!(
                state.text,
                "CMD depth={depth} pc={command_pc:#08x} op={opcode:#04x} w0={w0:#010x} \
                 w1={w1:#010x}{}",
                reference
                    .as_deref()
                    .map(|value| format!(" {value}"))
                    .unwrap_or_default(),
            )
            .expect("writing a display-list trace to String cannot fail");

            match opcode {
                G_MOVEWORD => {
                    let index = ((w0 >> 16) & 0xFF) as u16;
                    let offset = (w0 & 0xFFFF) as u16;
                    if index == G_MW_SEGMENT {
                        let segment = (offset / 4) as usize;
                        if segment < state.segments.len() {
                            state.segments[segment] = w1 & 0x00FF_FFFF;
                            writeln!(
                                state.text,
                                "SEG depth={depth} segment={segment} base={:#08x}",
                                state.segments[segment]
                            )
                            .expect("writing a display-list trace to String cannot fail");
                        }
                    }
                }
                G_DL => {
                    let is_branch = ((w0 >> 16) & 1) != 0;
                    if is_branch {
                        pc = resolve_addr(&state.segments, w1);
                        writeln!(state.text, "BRANCH depth={depth} target={pc:#08x}")
                            .expect("writing a display-list trace to String cannot fail");
                    } else if depth < MAX_DL_DEPTH {
                        trace_stream(rdram, w1, depth + 1, state);
                    } else {
                        writeln!(
                            state.text,
                            "STOP depth={depth} call_depth={MAX_DL_DEPTH} exceeded"
                        )
                        .expect("writing a display-list trace to String cannot fail");
                    }
                }
                G_ENDDL => {
                    writeln!(state.text, "RETURN depth={depth}")
                        .expect("writing a display-list trace to String cannot fail");
                    break;
                }
                _ => {}
            }
        }
    }

    let mut state = TraceState {
        segments: [0; 16],
        commands: 0,
        opcodes: BTreeMap::new(),
        text: String::new(),
    };
    trace_stream(rdram, dl_addr, 0, &mut state);
    writeln!(state.text, "SUMMARY commands={}", state.commands)
        .expect("writing a display-list trace to String cannot fail");
    for (opcode, count) in state.opcodes {
        writeln!(state.text, "OPCODE op={opcode:#04x} count={count}")
            .expect("writing a display-list trace to String cannot fail");
    }
    state.text
}

fn decode_stream(
    rdram: &mut [u8],
    dl_addr: u32,
    state: &mut DecodeState,
    rsp_memory: Option<&mut fn64_runtime::RspMemory>,
    ucode_catalog: Option<&F3dex2UcodeCatalog>,
    family: &mut GeometryWireFamily,
) {
    decode_stream_impl(
        rdram,
        dl_addr,
        state,
        false,
        rsp_memory,
        ucode_catalog,
        family,
    );
}

/// Apply public F3DEX2 `gSPDmaRead`/`gSPDmaWrite` wire semantics. The header
/// names these as debug transfers; the SGI RSP Programmer's Guide, chapter 4
/// tables 4-1/4-6, defines READ as DRAM -> I/DMEM and WRITE as
/// I/DMEM -> DRAM. Its DMA section requires 64-bit-aligned addresses and a
/// 64-bit-multiple length, with a maximum 4 KiB transfer. Those malformed
/// cases trap here instead of being rounded into a different transfer.
fn execute_dma_io(
    rdram: &mut [u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    segments: &[u32; 16],
    w0: u32,
    w1: u32,
) {
    assert_eq!(
        w0 & 0x0000_1000,
        0,
        "G_DMA_IO reserved wire bit 12 must be zero"
    );
    let write_to_dram = w0 & 0x0080_0000 != 0;
    let rsp_address = ((w0 >> 13) & 0x03ff) * 8;
    let size = usize::try_from((w0 & 0x0fff) + 1).expect("G_DMA_IO size fits usize");
    assert!(
        size.is_multiple_of(8),
        "G_DMA_IO transfer size {size} is not a 64-bit multiple"
    );
    let dram_address = resolve_addr(segments, w1);
    assert!(
        dram_address.is_multiple_of(8),
        "G_DMA_IO DRAM address {dram_address:#010x} is not 64-bit aligned"
    );
    let dram_end = dram_address
        .checked_add(size)
        .expect("G_DMA_IO DRAM range overflow");
    assert!(
        dram_end <= rdram.len(),
        "G_DMA_IO DRAM range {dram_address:#010x}..{dram_end:#010x} exceeds RDRAM length {:#x}",
        rdram.len()
    );

    let rsp_address = fn64_runtime::RspMemAddr::from_register(rsp_address);
    if write_to_dram {
        let bytes = rsp_memory
            .read_bytes(rsp_address, size)
            .unwrap_or_else(|error| panic!("G_DMA_IO gSPDmaWrite cannot read RSP memory: {error}"));
        fn64_runtime::RdramViewMut::from_storage(rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(dram_address as u32),
            &bytes,
        );
    } else {
        let mut bytes = vec![0; size];
        fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(dram_address as u32),
            &mut bytes,
        );
        rsp_memory
            .write_bytes(rsp_address, &bytes)
            .unwrap_or_else(|error| panic!("G_DMA_IO gSPDmaRead cannot write RSP memory: {error}"));
    }
}

/// Apply the public F3DEX2 `gSPLoadUcodeEx` compound command to the live RSP
/// memories. `G_RDPHALF_1` supplies the physical data-section address;
/// `G_LOAD_UCODE` supplies `(data_size - 1)` and the physical text address.
/// Public `OSTask` guidance fixes the text section at `SP_UCODE_SIZE` (4 KiB),
/// while the RSP Programmer's Guide states that a microcode `.dat` section is
/// loaded at the beginning of DMEM. Both sources are physical, not segmented,
/// addresses, and all SP DMA operands obey the hardware's 64-bit granularity.
fn execute_load_ucode(
    rdram: &[u8],
    rsp_memory: &mut fn64_runtime::RspMemory,
    w0: u32,
    text_address: u32,
    data_address: u32,
) -> UcodeDigest {
    assert_eq!(
        w0 & 0x00ff_0000,
        0,
        "G_LOAD_UCODE reserved wire bits 16..23 must be zero"
    );
    let data_size =
        usize::try_from((w0 & 0xffff) + 1).expect("G_LOAD_UCODE data-section size fits usize");
    assert!(
        data_size <= fn64_runtime::RSP_MEMORY_BANK_SIZE,
        "G_LOAD_UCODE data-section size {data_size} exceeds the 4 KiB DMEM bank"
    );
    assert!(
        data_size.is_multiple_of(8),
        "G_LOAD_UCODE data-section size {data_size} is not a 64-bit multiple"
    );
    assert!(
        text_address.is_multiple_of(8),
        "G_LOAD_UCODE text address {text_address:#010x} is not 64-bit aligned"
    );
    assert!(
        data_address.is_multiple_of(8),
        "G_LOAD_UCODE data address {data_address:#010x} is not 64-bit aligned"
    );

    let checked_source = |name: &str, address: u32, size: usize| {
        let start = usize::try_from(address).expect("physical RDRAM address fits usize");
        let end = start
            .checked_add(size)
            .unwrap_or_else(|| panic!("G_LOAD_UCODE {name} RDRAM range overflow"));
        assert!(
            end <= rdram.len(),
            "G_LOAD_UCODE {name} RDRAM range {start:#010x}..{end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        );
        start
    };
    let data_start = checked_source("data", data_address, data_size);
    let text_start = checked_source("text", text_address, SP_UCODE_SIZE);

    let mut data = vec![0; data_size];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(data_start as u32),
        &mut data,
    );
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
            &data,
        )
        .unwrap_or_else(|error| panic!("G_LOAD_UCODE cannot load DMEM data section: {error}"));

    let mut text = vec![0; SP_UCODE_SIZE];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(text_start as u32),
        &mut text,
    );
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &text,
        )
        .unwrap_or_else(|error| panic!("G_LOAD_UCODE cannot load IMEM text section: {error}"));
    UcodeDigest::from_text(&text)
}

fn decode_stream_impl(
    rdram: &mut [u8],
    dl_addr: u32,
    state: &mut DecodeState,
    raw_rdp: bool,
    mut rsp_memory: Option<&mut fn64_runtime::RspMemory>,
    ucode_catalog: Option<&F3dex2UcodeCatalog>,
    family: &mut GeometryWireFamily,
) {
    let mut pc = resolve_addr(&state.segments, dl_addr);

    loop {
        let command_end = pc.checked_add(8).unwrap_or_else(|| {
            panic!(
                "{} display-list PC {pc:#010x} overflows the host address space",
                family.name()
            )
        });
        assert!(
            command_end <= rdram.len(),
            "{} display list is truncated at RDRAM {pc:#010x}: need 8 command bytes, rdram_bytes={}",
            family.name(),
            rdram.len()
        );
        state.cmds_decoded += 1;
        assert!(
            state.cmds_decoded <= MAX_DL_COMMANDS,
            "{} display list exceeded the {MAX_DL_COMMANDS}-command budget at RDRAM {pc:#010x}; cyclic or corrupt command graph",
            family.name()
        );
        // Recomp rdram is word-native (see read_u32): each command word is a
        // logical big-endian u32 stored host-native, NOT a flat big-endian
        // byte run.
        let command_pc = pc;
        let wire_w0 = read_u32(rdram, pc);
        let wire_w1 = read_u32(rdram, pc + 4);
        let (w0, w1) = if raw_rdp {
            (wire_w0, wire_w1)
        } else {
            if consume_line_triangle_noop(*family, wire_w0, wire_w1) {
                pc += 8;
                continue;
            }
            normalize_geometry_command(*family, wire_w0, wire_w1, command_pc)
        };
        let wire_opcode = (w0 >> 24) as u8;
        let triangle_opcode = wire_opcode & 0x3f;
        let opcode = if raw_rdp && matches!(triangle_opcode, 0x08..=0x0f) {
            triangle_opcode
        } else {
            wire_opcode
        };
        pc += 8;

        if !raw_rdp && family.is_line() && matches!(opcode, G_TRI2 | G_QUAD) {
            crate::render_unsupported_panic(
                "render.gbi.geometry.command",
                format!(
                    "unsupported {family:?} polygon command byte {:#04x} at RDRAM {command_pc:#010x}: line microcode admits only public G_LINE3D; w0={wire_w0:#010x} w1={wire_w1:#010x}",
                    wire_w0 >> 24
                ),
            );
        }

        match opcode {
            G_NOOP => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "G_NOOP reserved first-word payload must be zero at RDRAM {:#010x}",
                    pc - 8
                );
                // Public gDPNoOpTag deliberately carries an arbitrary tag in
                // w1. The untagged macro is the same command with tag zero.
            }
            G_SPNOOP => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "G_SPNOOP reserved first-word payload must be zero"
                );
                assert_eq!(w1, 0, "G_SPNOOP reserved second word must be zero");
            }
            opcode @ (0x08..=0x0f) if raw_rdp => {
                let command_pc = pc - 8;
                let coefficients = decode_rdp_edge_coefficients(rdram, command_pc)
                    .expect("validated raw RDP edge triangle became truncated during decode");
                let shade_coefficients = (opcode & 4 != 0).then(|| {
                    decode_rdp_shade_coefficients(rdram, command_pc + 32)
                        .expect("validated raw RDP shade triangle became truncated during decode")
                });
                let shade_bytes = if opcode & 4 != 0 { 64 } else { 0 };
                let texture_coefficients = (opcode & 2 != 0).then(|| {
                    decode_rdp_texture_coefficients(rdram, command_pc + 32 + shade_bytes)
                        .expect("validated raw RDP texture triangle became truncated during decode")
                });
                let z_coefficients = (opcode & 1 != 0).then(|| {
                    let texture_bytes = if opcode & 2 != 0 { 64 } else { 0 };
                    decode_rdp_z_coefficients(rdram, command_pc + 32 + shade_bytes + texture_bytes)
                        .expect("validated raw RDP Z triangle became truncated during decode")
                });
                let texture = (opcode & 2 != 0)
                    .then(|| {
                        bind_texture_set(
                            &state.tex,
                            coefficients.tile,
                            coefficients.level,
                            state.other_mode.texture_lut(),
                        )
                    })
                    .flatten();
                assert!(
                    opcode & 2 == 0 || texture.is_some(),
                    "raw RDP textured triangle references tile {} without a decoded G_LOADBLOCK/G_LOADTILE image",
                    coefficients.tile
                );
                state.ops.push(RenderOp::RawTriangle(RawRdpTriangle {
                    edge: coefficients,
                    shade: shade_coefficients,
                    texture_coefficients,
                    z: z_coefficients,
                    texture,
                    other_mode: state.other_mode,
                    combiner: state.combiner,
                    blender: active_blender(state),
                    scissor: state.scissor,
                }));
                pc += raw_rdp_command_width(opcode).expect("raw triangle width") as usize - 8;
            }
            G_RDPHALF_1 => {
                // Public gbi.h composes BranchLessZ as G_RDPHALF_1(target)
                // followed by G_BRANCH_Z(vertex offsets, depth threshold).
                state.rdp_half_1 = Some(w1);
            }
            G_CULLDL => {
                // F3DEX2 gSPCullDisplayList packs inclusive cache indices as
                // v*2 in the low 16 bits of each word. The microcode ANDs
                // their retained clipping codes; any common side terminates
                // this display list exactly like G_ENDDL.
                let encoded_start = (w0 & 0xffff) as usize;
                let encoded_end = (w1 & 0xffff) as usize;
                assert!(
                    encoded_start.is_multiple_of(2) && encoded_end.is_multiple_of(2),
                    "G_CULLDL cache indices must use F3DEX2 v*2 encoding: {encoded_start:#06x}..={encoded_end:#06x}"
                );
                let start = encoded_start / 2;
                let end = encoded_end / 2;
                let cache_capacity = family.cache_capacity();
                assert!(
                    start < end && end < cache_capacity,
                    "{} G_CULLDL inclusive cache range {start}..={end} must satisfy 0 <= start < end < {cache_capacity}",
                    family.name()
                );
                let common_code = state.vtx_cache[start..=end]
                    .iter()
                    .fold(u8::MAX, |common, vertex| common & vertex.clip_code);
                if common_code != 0 {
                    break;
                }
            }
            G_BRANCH_Z => {
                // gSPBranchLessZraw redundantly packs the same cache slot as
                // v*5 (vertex record offset) and v*2 (screen-Z offset).
                let encoded_vertex = ((w0 >> 12) & 0x0fff) as usize;
                let encoded_z = (w0 & 0x0fff) as usize;
                assert!(
                    encoded_vertex.is_multiple_of(5) && encoded_z.is_multiple_of(2),
                    "G_BRANCH_Z malformed vertex offsets v*5={encoded_vertex:#05x} v*2={encoded_z:#05x}"
                );
                let vertex_slot = encoded_vertex / 5;
                let z_slot = encoded_z / 2;
                assert_eq!(
                    vertex_slot, z_slot,
                    "G_BRANCH_Z vertex offsets select different cache slots {vertex_slot} and {z_slot}"
                );
                let cache_capacity = family.cache_capacity();
                assert!(
                    vertex_slot < cache_capacity,
                    "{} G_BRANCH_Z cache slot {vertex_slot} is outside slots 0..={}",
                    family.name(),
                    cache_capacity - 1
                );
                let vertex = &state.vtx_cache[vertex_slot];
                if vertex.z_screen <= w1 {
                    let target = state.rdp_half_1.unwrap_or_else(|| {
                        panic!("G_BRANCH_Z reached without a preceding G_RDPHALF_1 target")
                    });
                    pc = resolve_addr(&state.segments, target);
                    continue;
                }
            }
            G_VTX => {
                // F3DEX2 G_VTX (F3DEX2-CONCEPTS.md §2.1): the RSP-side wire
                // layout is n = field(w0,12,8), end-index = field(w0,1,7),
                // and the destination start slot v0 = end - n. w1 = segmented
                // vertex-array address. (NOT the F3DEX/SDK-macro `/2` form,
                // which misplaces vertices -- failure risk #2.)
                let n = ((w0 >> 12) & 0xFF) as usize;
                let end = ((w0 >> 1) & 0x7F) as usize;
                let cache_capacity = family.cache_capacity();
                let max_load = family.max_vertex_load_count();
                assert!(
                    (1..=max_load).contains(&n),
                    "{} G_VTX count {n} must be in 1..={max_load} at RDRAM {:#010x}",
                    family.name(),
                    pc - 8
                );
                assert!(
                    end <= cache_capacity && end >= n,
                    "{} G_VTX encoded end slot {end} and count {n} do not select a cache range within slots 0..={} at RDRAM {:#010x}",
                    family.name(),
                    cache_capacity - 1,
                    pc - 8
                );
                let v0 = end - n;
                load_vertices(rdram, state, w1, n, v0, *family);
            }
            G_MODIFYVTX => {
                // Public F3DEX2 gSPModifyVertex packs `where` in w0[23:16],
                // cache slot * 2 in w0[15:0], and the replacement in w1.
                // Values are already post-transform cache values: RGBA bytes,
                // signed S10.5 ST, signed S13.2 screen XY, or unsigned 16.16
                // screen Z. In particular ST is not multiplied by G_TEXTURE
                // here; the manual requires callers to provide that scaled
                // value themselves.
                let where_field = ((w0 >> 16) & 0xFF) as u8;
                let encoded_slot = (w0 & 0xFFFF) as usize;
                assert!(
                    encoded_slot.is_multiple_of(2),
                    "G_MODIFYVTX cache index encoding {encoded_slot:#06x} is not divisible by two"
                );
                let slot = encoded_slot / 2;
                let cache_capacity = family.cache_capacity();
                assert!(
                    slot < cache_capacity,
                    "{} G_MODIFYVTX cache slot {slot} is outside slots 0..={}",
                    family.name(),
                    cache_capacity - 1
                );
                let vertex = &mut state.vtx_cache[slot];
                match where_field {
                    G_MWO_POINT_RGBA => {
                        [vertex.r, vertex.g, vertex.b, vertex.a] = w1.to_be_bytes();
                    }
                    G_MWO_POINT_ST => {
                        let [s_hi, s_lo, t_hi, t_lo] = w1.to_be_bytes();
                        vertex.s = i16::from_be_bytes([s_hi, s_lo]) as f32 / 32.0;
                        vertex.t = i16::from_be_bytes([t_hi, t_lo]) as f32 / 32.0;
                    }
                    G_MWO_POINT_XYSCREEN => {
                        let [x_hi, x_lo, y_hi, y_lo] = w1.to_be_bytes();
                        vertex.x = i16::from_be_bytes([x_hi, x_lo]) as f32 / 4.0;
                        vertex.y = i16::from_be_bytes([y_hi, y_lo]) as f32 / 4.0;
                        vertex.clip_position = None;
                    }
                    G_MWO_POINT_ZSCREEN => {
                        vertex.z = w1 as f32 / 65536.0;
                        vertex.z_screen = w1;
                        vertex.clip_position = None;
                    }
                    _ => crate::render_unsupported_panic(
                        "render.gbi.geometry.modify-vtx",
                        format!(
                            "G_MODIFYVTX cache slot {slot} uses unsupported where field {where_field:#04x}"
                        ),
                    ),
                }
            }
            G_LINE3D => {
                // Public F3DEX2 gbi.h packs v0*2, v1*2, and the half-pixel
                // width increment into w0[23:16], w0[15:8], and w0[7:0].
                // The flat-shade flag is already expressed by swapping the
                // two encoded endpoints; w1 is reserved and emitted as zero.
                assert_eq!(w1, 0, "G_LINE3D reserved second word must be zero");
                let encoded = [((w0 >> 16) & 0xff) as usize, ((w0 >> 8) & 0xff) as usize];
                assert!(
                    encoded.iter().all(|value| value.is_multiple_of(2)),
                    "G_LINE3D cache indices must use F3DEX2 v*2 encoding: {} and {}",
                    encoded[0],
                    encoded[1]
                );
                let slots = [encoded[0] / 2, encoded[1] / 2];
                let width_parameter = (w0 & 0xff) as u8;
                if let Some(line) = resolve_line(
                    &state.vtx_cache,
                    slots,
                    width_parameter,
                    LineDecodeSnapshot {
                        smooth_shading: state.geometry_mode & G_SHADING_SMOOTH != 0,
                        texture: active_texture(&state.tex, state.other_mode),
                        other_mode: state.other_mode,
                        combiner: state.combiner,
                        blender: active_blender(state),
                        scissor: state.scissor,
                        viewport: state.viewport,
                        clip_ratio: state.clip_ratio,
                    },
                ) {
                    state.ops.push(RenderOp::Line(line));
                }
            }
            G_TRI1 => {
                // F3DEX2 G_TRI1 (F3DEX2-CONCEPTS.md §2.2): three 7-bit
                // vertex-cache-slot fields in w0 at bits 17/9/1 -- each is
                // already the slot (0-31), no /2 needed.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex, state.other_mode);
                let blender = active_blender(state);
                let idx = tri_indices(w0);
                if let Some(mut t) = resolve_tri_for_family(
                    &state.vtx_cache,
                    idx,
                    *family,
                    state.geometry_mode,
                    state.clip_ratio,
                    cull,
                    texture,
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.ops.push(RenderOp::Triangle(t));
                }
            }
            G_TRI2 | G_QUAD => {
                // F3DEX2 G_TRI2 / G_QUAD (§2.3): triangle A's three 7-bit
                // slot fields in w0 (bits 17/9/1), triangle B's in w1 at the
                // SAME bit positions. G_QUAD decodes identically to G_TRI2.
                let cull = cull_mode_from(state.geometry_mode);
                let texture = active_texture(&state.tex, state.other_mode);
                let blender = active_blender(state);
                let idx_a = tri_indices(w0);
                let idx_b = tri_indices(w1);
                if let Some(mut t) = resolve_tri_for_family(
                    &state.vtx_cache,
                    idx_a,
                    *family,
                    state.geometry_mode,
                    state.clip_ratio,
                    cull,
                    texture.clone(),
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.ops.push(RenderOp::Triangle(t));
                }
                if let Some(mut t) = resolve_tri_for_family(
                    &state.vtx_cache,
                    idx_b,
                    *family,
                    state.geometry_mode,
                    state.clip_ratio,
                    cull,
                    texture,
                    state.other_mode,
                    state.combiner,
                    blender,
                ) {
                    t.scissor = state.scissor;
                    state.ops.push(RenderOp::Triangle(t));
                }
            }
            G_MTX => {
                // F3DEX2 gsSPMatrix (gbi.h ~2106): w0 = op<<24 |
                // ((len-1)/8)<<19 | (ofs/8)<<8 | idx; the low byte on the
                // wire is `idx = params ^ G_MTX_PUSH`. F3DEX_GBI_2 param bits
                // (gbi.h:233-239): PROJECTION=0x04, LOAD=0x02, PUSH=0x01.
                // Un-XOR the push bit to recover the caller's params. w1 =
                // segmented matrix address.
                let wire_idx = (w0 & 0xFF) as u8;
                let destination_offset_div8 = ((w0 >> 8) & 0xFF) as u8;
                let length_div8_minus_one = ((w0 >> 19) & 0x1F) as u8;
                assert_eq!(
                    destination_offset_div8, 0,
                    "G_MTX destination offset/8 must be zero"
                );
                assert_eq!(length_div8_minus_one, 7, "G_MTX must carry one 64-byte Mtx");
                assert_eq!(
                    wire_idx & !0x07,
                    0,
                    "G_MTX wire parameter {wire_idx:#04x} contains non-public flag bits"
                );
                let params = wire_idx ^ 0x01; // ^ G_MTX_PUSH
                let is_projection = params & 0x04 != 0; // G_MTX_PROJECTION
                let is_load = params & 0x02 != 0; // G_MTX_LOAD
                let is_push = params & 0x01 != 0; // G_MTX_PUSH
                let addr = resolve_addr(&state.segments, w1);
                let mtx = read_mtx(rdram, addr).unwrap_or_else(|| {
                    panic!(
                        "G_MTX reads past RDRAM: source={addr:#x}, bytes=64, rdram_bytes={}",
                        rdram.len()
                    )
                });
                {
                    #[cfg(not(test))]
                    if projdump::on() {
                        eprintln!(
                            "[FN64_DUMP_PROJ] G_MTX proj={} load={} push={} @rdram=0x{addr:06x} seg_w1=0x{w1:08x} mv_depth={} rows=[{:?} | {:?} | {:?} | {:?}]",
                            is_projection,
                            is_load,
                            is_push,
                            state.mv_stack.len(),
                            mtx[0],
                            mtx[1],
                            mtx[2],
                            mtx[3]
                        );
                    }
                    if is_projection {
                        // The projection matrix ALSO honors LOAD vs MUL. OoT
                        // loads the perspective matrix once with LOAD, then
                        // concatenates the camera/view matrix onto it with
                        // PROJECTION|MUL (guLookAt output). Treating every
                        // projection G_MTX as a LOAD (a prior bug) let the
                        // view matrix -- whose 4th row is [0,0,0,1], no
                        // projective term -- OVERWRITE the real perspective
                        // matrix (4th row [0,0,-1,0]).
                        //
                        // MUL ORDER (hardware/RT64): the incoming matrix
                        // multiplies on the LEFT of the accumulated
                        // projection -- `viewProj = new * viewProj` (RT64
                        // rt64_rsp.cpp:171). So the perspective LOAD gives
                        // `proj = P`, then the view MUL gives `proj = V * P`,
                        // and the final MVP below is `M * (V * P)`. This is
                        // the row-vector hardware product built column-major
                        // for our column-vector `transform_point`.
                        state.proj = Some(if is_load {
                            mtx
                        } else {
                            match state.proj {
                                Some(p) => mat_mul(&mtx, &p),
                                None => mtx,
                            }
                        });
                    } else {
                        // Modelview: a PUSH saves the current top so a later
                        // G_POPMTX restores it. LOAD replaces, MUL
                        // concatenates. MUL puts the incoming matrix on the
                        // LEFT (`modelview = new * modelview`, RT64
                        // rt64_rsp.cpp:197) so successive object transforms
                        // compose in the same order the hardware applies them.
                        if is_push {
                            state.mv_stack.push(state.modelview);
                        }
                        if is_load {
                            state.modelview = mtx;
                        } else {
                            state.modelview = mat_mul(&mtx, &state.modelview);
                        }
                    }
                    recompute_mvp(state);
                }
            }
            G_POPMTX => {
                // F3DEX2 gsSPPopMatrixN encodes the requested count as a
                // byte address `num * 64` in w1. Only the modelview stack is
                // public for this command.
                #[cfg(not(test))]
                if projdump::on() {
                    eprintln!(
                        "[FN64_DUMP_PROJ] G_POPMTX mv_depth_before={}",
                        state.mv_stack.len()
                    );
                }
                assert!(
                    w1.is_multiple_of(64) && w1 != 0,
                    "G_POPMTX count address {w1:#010x} must be a nonzero multiple of 64"
                );
                let count = (w1 / 64) as usize;
                assert!(
                    count <= state.mv_stack.len(),
                    "G_POPMTX requests {count} entries from modelview depth {}",
                    state.mv_stack.len()
                );
                for _ in 0..count {
                    state.modelview = state
                        .mv_stack
                        .pop()
                        .expect("validated G_POPMTX depth changed during pop");
                }
                recompute_mvp(state);
            }
            G_DMA_IO => {
                let rsp_memory = rsp_memory.as_deref_mut().unwrap_or_else(|| {
                    panic!("G_DMA_IO requires execute_display_list_f3dex2_ops with live RSP memory")
                });
                execute_dma_io(rdram, rsp_memory, &state.segments, w0, w1);
            }
            G_LOAD_UCODE => {
                let loading_family = *family;
                let rsp_memory = rsp_memory.as_deref_mut().unwrap_or_else(|| {
                    panic!(
                        "G_LOAD_UCODE requires execute_display_list_f3dex2_ops with live RSP memory"
                    )
                });
                let data_address = state.rdp_half_1.unwrap_or_else(|| {
                    panic!(
                        "G_LOAD_UCODE reached without the compound command's preceding G_RDPHALF_1 data address"
                    )
                });
                let digest = execute_load_ucode(rdram, rsp_memory, w0, w1, data_address);
                if loading_family.is_legacy_loadable() {
                    reset_legacy_rsp_state_from_ucode_load(state);
                } else {
                    reset_rsp_state_from_ucode_load(state);
                }
                if let Some(catalog) = ucode_catalog {
                    if let Some(next_family) = catalog.family(digest) {
                        *family = next_family;
                        initialize_geometry_family_state(state, next_family);
                    } else {
                        state.unsupported_ucode_reload = Some(digest);
                        break;
                    }
                }
            }
            G_MOVEWORD => {
                // F3DEX2 gsMoveWd (gbi.h ~2267): w0 = op<<24 | index<<16 |
                // offset<<0 (16-bit offset); w1 = data. Segment table write
                // is index==G_MW_SEGMENT, segment number = offset/4, base =
                // w1 (masked to a physical rdram offset).
                let index = ((w0 >> 16) & 0xFF) as u16;
                let offset = (w0 & 0xFFFF) as u16;
                if index == G_MW_SEGMENT {
                    assert!(
                        offset.is_multiple_of(4),
                        "G_MOVEWORD G_MW_SEGMENT offset {offset:#06x} is not word aligned"
                    );
                    let seg = (offset / 4) as usize;
                    assert!(
                        seg < state.segments.len(),
                        "G_MOVEWORD G_MW_SEGMENT index {seg} exceeds segments 0..=15"
                    );
                    // Base is a physical rdram address; strip any KSEG high
                    // bits, keep the low 24 (segments span rdram).
                    state.segments[seg] = w1 & 0x00FF_FFFF;
                } else if index == G_MW_NUMLIGHT {
                    // F3DEX2 gsSPNumLights (gbi.h:2887): data = NUML(n) =
                    // n*24, so the directional-light count is w1/24. The
                    // ambient light lives in slot `num_dir` (gbi.h:2902:
                    // "the highest numbered light is always the ambient").
                    assert_eq!(offset, 0, "G_MOVEWORD G_MW_NUMLIGHT offset must be zero");
                    assert!(
                        w1.is_multiple_of(24),
                        "G_MOVEWORD G_MW_NUMLIGHT value {w1} is not a 24-byte light stride"
                    );
                    let n = (w1 / 24) as usize;
                    assert!(
                        n < MAX_LIGHTS,
                        "G_MOVEWORD G_MW_NUMLIGHT directional count {n} exceeds seven"
                    );
                    state.lights.num_dir = n;
                } else if index == G_MW_CLIP {
                    if family.is_reject() {
                        let ratio = w1 as u16;
                        assert!(
                            !matches!(ratio, 1 | 0xffff),
                            "F3DLX.Rej reject-box ratio must be public FRUSTRATIO_2..6"
                        );
                    }
                    state.clip_ratio.write(offset, w1);
                } else if index == G_MW_LIGHTCOL {
                    // Public gSPLightColor emits the same RGBA word to the
                    // primary and copied color destinations. Alpha is ignored;
                    // neither write changes the retained light direction.
                    let slot = light_slot_from_moveword_offset(offset).unwrap_or_else(|| {
                        panic!(
                            "G_MOVEWORD G_MW_LIGHTCOL offset {offset:#06x} is not a public F3DEX2 light-color destination"
                        )
                    });
                    set_light_color(state, slot, w1);
                } else if index == G_MW_FOG {
                    assert_eq!(
                        offset, 0,
                        "G_MOVEWORD G_MW_FOG offset must be G_MWO_FOG (zero)"
                    );
                    state.fog = FogFactor {
                        multiplier: (w1 >> 16) as u16 as i16,
                        offset: w1 as u16 as i16,
                    };
                } else if index == G_MW_FORCEMTX {
                    assert_eq!(offset, 0, "G_MOVEWORD G_MW_FORCEMTX offset must be zero");
                    assert_eq!(
                        w1, 0x0001_0000,
                        "G_MOVEWORD G_MW_FORCEMTX marker must be 0x00010000"
                    );
                    state.mvp = Some(state.pending_forced_mvp.take().expect(
                        "G_MOVEWORD G_MW_FORCEMTX requires a preceding G_MOVEMEM G_MV_MATRIX",
                    ));
                } else if index == G_MW_PERSPNORM {
                    assert_eq!(offset, 0, "G_MOVEWORD G_MW_PERSPNORM offset must be zero");
                    assert_eq!(
                        w1 & 0xffff_0000,
                        0,
                        "G_MOVEWORD G_MW_PERSPNORM scale must be a public u16 value"
                    );
                    state.persp_normalize = PerspectiveNormalize(Some(w1 as u16));
                } else {
                    crate::render_unsupported_panic(
                        "render.gbi.geometry.moveword",
                        format!(
                            "G_MOVEWORD unsupported index {index:#04x} offset {offset:#06x}: w0={w0:#010x} w1={w1:#010x}"
                        ),
                    );
                }
            }
            G_DL => {
                // F3DEX2 gsSPDisplayList / gsSPBranchList (gbi.h ~2174-2178):
                // both pack via gDma1p(G_DL, dl, 0, p) so w0 = op<<24 |
                // p<<16, w1 = segmented address of the target DL. The `p`
                // byte at bits 16-23 is the push flag: G_DL_PUSH=0 (gbi.h:966)
                // is a CALL (push a return address, resume the caller after
                // the callee's G_ENDDL); G_DL_NOPUSH=1 (gbi.h:967) is a
                // BRANCH/tail-jump (gsSPBranchList) that REPLACES the current
                // DL pointer -- the target runs in place of the rest of this
                // stream and there is NO return to the bytes after the branch.
                //
                // BUG FIXED HERE: previously both cases recursed and then
                // *continued* decoding the current stream after return. For a
                // BRANCH that is wrong -- the words after a gsSPBranchList are
                // not commands (typically zero-fill or the next unrelated
                // buffer), so the decoder walked straight into garbage and
                // every trailing byte became a bogus "unrecognized opcode",
                // cascading the whole frame into ~14K junk skips (proven from
                // a live OoT gameplay task: the root DL's first command is a
                // gsSPBranchList `w0=0xde01_0000` whose trailing bytes are all
                // zero). We now recurse into the target and then STOP the
                // current stream for a branch (mirroring RT64's runDl, which
                // only pushes a return address when the push bit is clear).
                let is_branch = ((w0 >> 16) & 0x01) != 0; // G_DL_NOPUSH
                if is_branch {
                    // Tail branch: the target REPLACES the current DL
                    // pointer -- on hardware this consumes NO return-stack
                    // entry, so it must not recurse or count against
                    // MAX_DL_DEPTH (OoT chains branch lists deeper than any
                    // fixed cap; the old recursing version falsely tripped
                    // it). A self-referencing branch cycle is bounded by
                    // MAX_DL_COMMANDS at the loop top.
                    pc = resolve_addr(&state.segments, w1);
                    continue;
                }
                if state.dl_depth < MAX_DL_DEPTH {
                    // NOTE: G_DL is a pure address call/return -- it does NOT
                    // save or restore the matrix stack. The RSP's modelview/
                    // projection state is GLOBAL across a nested DL; only
                    // G_MTX (with G_MTX_PUSH) and G_POPMTX push/pop matrices.
                    // A previous version wrapped the recursion in a
                    // modelview push/pop, which corrupted transforms after a
                    // nested DL returned -- gameplay geometry (deeply nested
                    // DLs) then projected to ±100k px off-screen. We now
                    // recurse with shared global matrix state, exactly like
                    // the hardware call/return (RT64 push/popReturnAddress
                    // only saves the DL pointer, never the matrix).
                    state.dl_depth += 1;
                    decode_stream(
                        rdram,
                        w1,
                        state,
                        rsp_memory.as_deref_mut(),
                        ucode_catalog,
                        family,
                    );
                    state.dl_depth -= 1;
                    if state.unsupported_ucode_reload.is_some() {
                        break;
                    }
                } else {
                    panic!(
                        "G_DL call at RDRAM {:#010x} exceeds the {MAX_DL_DEPTH}-entry display-list stack",
                        pc - 8
                    );
                }
            }
            G_TEXTURE => {
                // F3DEX2 gsSPTexture (§5.2): on-bit field(w0,1,7), tile
                // field(w0,8,3), max-level field(w0,11,3), S scale field(w1,16,16), T scale
                // field(w1,0,16) (both U0.16). Latch enable + tile + scale so
                // the next G_LOAD*/G_TRI can bind + address a texture.
                let on = ((w0 >> 1) & 0x7F) != 0;
                let tile = ((w0 >> 8) & 0x07) as u8;
                let scale_s = ((w1 >> 16) & 0xFFFF) as f32 / 65536.0;
                let scale_t = (w1 & 0xFFFF) as f32 / 65536.0;
                state.tex.tex_enabled = on;
                state.tex.tex_tile = tile;
                state.tex.tex_max_level = ((w0 >> 11) & 0x07) as u8;
                state.tex.tex_scale_s = scale_s;
                state.tex.tex_scale_t = scale_t;
            }
            G_RDPSETOTHERMODE => {
                // Full expert-mode write: high 24 bits live in w0's payload,
                // low 32 bits in w1 (gbi.h:3697-3737). OoT's setup DLs use
                // this path as well as the F3DEX2 partial setters.
                state.other_mode.high = w0 & 0x00FF_FFFF;
                state.other_mode.low = w1;
            }
            G_SETOTHERMODE_H => {
                // F3DEX2 gSPSetOtherMode (`gbi.h:3353-3369`) stores
                // `32-shift-len` at w0[15:8] and `len-1` at w0[7:0]. Rebuild
                // the selected H mask and preserve every other bit, matching
                // RT64's decode/update split (`rt64_gbi_f3dex2.cpp:24-33`,
                // `rt64_rsp.cpp:1026-1037`).
                state.other_mode.high = update_other_mode_word(state.other_mode.high, w0, w1)
                    .unwrap_or_else(|| {
                        panic!(
                            "malformed G_SETOTHERMODE_H range at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                            pc - 8
                        )
                    });
            }
            G_SETOTHERMODE_L => {
                state.other_mode.low = update_other_mode_word(state.other_mode.low, w0, w1)
                    .unwrap_or_else(|| {
                        panic!(
                            "malformed G_SETOTHERMODE_L range at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                            pc - 8
                        )
                    });
            }
            G_SETBLENDCOLOR => {
                // Public gbi.h:3646-3650 packs RGBA into w1, alpha in bits
                // 7..0. Threshold alpha compare uses precisely this component
                // (OoT z_rcp.c:815-835; RT64 RasterPS.hlsl:209-211).
                state.other_mode.blend_color_alpha = w1 as u8;
                state.blend_color = w1.to_be_bytes();
            }
            G_SETFOGCOLOR => state.fog_color = w1.to_be_bytes(),
            G_SETKEYGB => state.combiner.key.set_gb(w0, w1),
            G_SETKEYR => state.combiner.key.set_r(w1),
            G_SETCONVERT => {
                state.combiner.convert = ConvertState::decode(w0, w1);
            }
            G_SETCIMG => {
                // Public gDPSetColorImage packing: format[23:21], size[20:19],
                // width-1[11:0], and image address in w1. The F3DEX2 command
                // processor resolves segmented addresses before the RDP sees
                // them; this decoder performs the same mapping explicitly.
                state.ops.push(RenderOp::SetColorImage(ColorImage {
                    format: ((w0 >> 21) & 0x07) as u8,
                    size: ((w0 >> 19) & 0x03) as u8,
                    width: ((w0 & 0x0fff) + 1) as u16,
                    address: u32::try_from(resolve_addr(&state.segments, w1))
                        .expect("resolved color-image address exceeds u32"),
                }));
            }
            G_SETZIMG => {
                state.ops.push(RenderOp::SetDepthImage(DepthImage {
                    address: u32::try_from(resolve_addr(&state.segments, w1))
                        .expect("resolved depth-image address exceeds u32"),
                }));
            }
            G_SETPRIMDEPTH => {
                // Public gDPSetPrimDepth uses the generic set-color packing:
                // Z in the high halfword, DeltaZ in the low halfword.
                state.ops.push(RenderOp::SetPrimitiveDepth(PrimitiveDepth {
                    z: (w1 >> 16) as u16,
                    delta_z: w1 as u16,
                }));
            }
            G_SETFILLCOLOR => {
                // gDPSetFillColor writes the raw 32-bit fill register. On a
                // 16-bit color image its high/low RGBA5551 halfwords alternate
                // across logical pixels.
                state.fill_color = w1;
            }
            G_FILLRECT => {
                // Public gDPFillRectangle packing uses lower-right in w0 and
                // upper-left in w1, all as unsigned quarter-pixel fields.
                // Fill-cycle lower-right coverage is inclusive; raster.rs
                // applies that rule together with the exclusive scissor.
                state.ops.push(RenderOp::FillRectangle(FillRectangle {
                    ulx: ((w1 >> 12) & 0x0fff) as f32 / 4.0,
                    uly: (w1 & 0x0fff) as f32 / 4.0,
                    lrx: ((w0 >> 12) & 0x0fff) as f32 / 4.0,
                    lry: (w0 & 0x0fff) as f32 / 4.0,
                    fill_color: state.fill_color,
                    cycle_type: state.other_mode.cycle_type(),
                    scissor: state.scissor,
                    other_mode: state.other_mode,
                    combiner: state.combiner,
                    blender: active_blender(state),
                }));
            }
            G_RDPLOADSYNC | G_RDPPIPESYNC | G_RDPTILESYNC => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "{} reserved first-word payload must be zero",
                    opcode_name(opcode)
                );
                // SGI RDP Command Summary Tables 1/32/33 assign no field to
                // this word. F3DEX2 macros generate zero and remain checked,
                // while the raw RDP stream admitted by DPC can retain an
                // unrelated word there; the hardware command has no input to
                // consume it.
                if !raw_rdp {
                    assert_eq!(
                        w1,
                        0,
                        "{} reserved second word must be zero",
                        opcode_name(opcode)
                    );
                }
            }
            G_RDPFULLSYNC => {
                assert_eq!(
                    w0 & 0x00ff_ffff,
                    0,
                    "G_RDPFULLSYNC reserved first-word payload must be zero"
                );
                if !raw_rdp {
                    assert_eq!(w1, 0, "G_RDPFULLSYNC reserved second word must be zero");
                }
                state.ops.push(RenderOp::FullSync);
            }
            G_SETTIMG => {
                // G_SETTIMG (§5.1): format field(w0,21,3), size field(w0,19,2),
                // width-1 field(w0,0,12), image addr w1 (segmented). Pointer +
                // format latch only; no texel data moves until a G_LOAD*.
                state.tex.timg_fmt = ((w0 >> 21) & 0x07) as u8;
                state.tex.timg_siz = ((w0 >> 19) & 0x03) as u8;
                state.tex.timg_width = ((w0 & 0x0fff) + 1) as u16;
                state.tex.timg_addr = w1;
            }
            G_SETTILE => {
                // G_SETTILE (§5.1): w0 = fmt field(w0,21,3), siz field(w0,19,2),
                // line field(w0,9,9), tmem field(w0,0,9); w1 = tile
                // field(w1,24,3), palette field(w1,20,4), cmT field(w1,18,2),
                // maskT field(w1,14,4), shiftT field(w1,10,4), cmS
                // field(w1,8,2), maskS field(w1,4,4), shiftS field(w1,0,4).
                // In each cm field bit0 enables mirror and bit1 enables clamp.
                let tile = ((w1 >> 24) & 0x07) as usize;
                apply_set_tile(&mut state.tex.tiles[tile], w0, w1);
            }
            G_SETTILESIZE => {
                // G_SETTILESIZE (§5.1): w0 = uls field(w0,12,12), ult
                // field(w0,0,12); w1 = tile field(w1,24,3), lrs field(w1,12,12),
                // lrt field(w1,0,12). Coords are S10.5 (÷4 for texel extent).
                let uls = ((w0 >> 12) & 0xFFF) as u16;
                let ult = (w0 & 0xFFF) as u16;
                let tile = ((w1 >> 24) & 0x07) as usize;
                let lrs = ((w1 >> 12) & 0xFFF) as u16;
                let lrt = (w1 & 0xFFF) as u16;
                let t = &mut state.tex.tiles[tile];
                t.uls = uls;
                t.ult = ult;
                t.lrs = lrs;
                t.lrt = lrt;
            }
            G_LOADTLUT => {
                // G_LOADTLUT (§5.1): load a CI palette from the latched TIMG
                // image. Public gbi.h packs `num - 1` directly into the
                // 10-bit field at bits 14..23. TLUT entries are 16-bit
                // RGBA5551 in RDRAM.
                let count = load_tlut_count(w1);
                let base = resolve_addr(&state.segments, state.tex.timg_addr);
                assert_texture_source_range(rdram, base, count - 1, G_IM_SIZ_16B, "G_LOADTLUT");
                let tile_index = ((w1 >> 24) & 0x07) as usize;
                let tmem_base = state.tex.tiles[tile_index].tmem;
                let mut tlut = Vec::with_capacity(count);
                for i in 0..count {
                    let px = read_u16(rdram, base + i * 2);
                    tlut.push(rgba5551_to_rgba8888(px));
                    std::rc::Rc::make_mut(&mut state.tex.tmem).write_tlut(tmem_base, i, px);
                }
                state.tex.tlut = tlut;
                let tile = &mut state.tex.tiles[tile_index];
                tile.uls = ((w0 >> 12) & 0x0fff) as u16;
                tile.ult = (w0 & 0x0fff) as u16;
                tile.lrs = ((w1 >> 12) & 0x0fff) as u16;
                tile.lrt = (w1 & 0x0fff) as u16;
            }
            G_LOADBLOCK | G_LOADTILE => {
                // G_LOADBLOCK / G_LOADTILE (§5.1): DMA source texels into the
                // physical 4 KiB TMEM image using this LOAD tile's base,
                // stride, size, and odd-row bank exchange. A later render
                // tile can reinterpret the same bytes with a different
                // format, extent, or tile number.
                let tile = ((w1 >> 24) & 0x07) as usize;
                if opcode == G_LOADTILE {
                    load_tile_into_tmem(rdram, &mut state.tex, &state.segments, tile, w0, w1);
                } else {
                    load_block_into_tmem(rdram, &mut state.tex, &state.segments, tile, w0, w1);
                }
            }
            G_MOVEMEM => {
                // F3DEX2 gsMoveMem (§1.4): w0 low byte = index (which RSP
                // block), field(w0,8,8) = offset/8, w1 = segmented source
                // address. G_MV_VIEWPORT (index 8) points at a 16-byte `Vp`;
                // G_MV_LIGHT (index 0x0a) addresses the two public LookAt
                // directions followed by the 16-byte directional/ambient
                // `Light` records. G_MV_MATRIX stages the public force-matrix
                // compound operation. Point indices remain unimplemented.
                let index = (w0 & 0xFF) as u8;
                let ofs_div8 = ((w0 >> 8) & 0xFF) as usize;
                let length_div8_minus_one = ((w0 >> 19) & 0x1f) as usize;
                if index == G_MV_VIEWPORT {
                    assert_eq!(
                        ofs_div8, 0,
                        "G_MOVEMEM G_MV_VIEWPORT destination offset must be zero"
                    );
                    assert_eq!(
                        length_div8_minus_one, 1,
                        "G_MOVEMEM G_MV_VIEWPORT must carry one 16-byte Vp"
                    );
                    let addr = resolve_addr(&state.segments, w1);
                    state.viewport = Some(read_viewport(rdram, addr).unwrap_or_else(|| {
                        panic!(
                            "G_MOVEMEM G_MV_VIEWPORT reads past RDRAM: source={addr:#x}, bytes=16, rdram_bytes={}",
                            rdram.len()
                        )
                    }));
                } else if index == G_MV_LIGHT {
                    assert_eq!(
                        length_div8_minus_one, 1,
                        "G_MOVEMEM G_MV_LIGHT must carry one 16-byte Light or LookAt record"
                    );
                    // Public F3DEX2 gbi.h assigns offsets 0*24 and 1*24 to
                    // LookAt X/Y. gsSPLight starts at 2*24; LIGHT_1 therefore
                    // maps to slot 0 after the two reserved entries.
                    let addr = resolve_addr(&state.segments, w1);
                    if ofs_div8 == 0 {
                        load_look_at(rdram, state, addr, LookAtAxis::X);
                    } else if ofs_div8 == 3 {
                        load_look_at(rdram, state, addr, LookAtAxis::Y);
                    } else if let Some(slot) = light_slot_from_movemem_offset(ofs_div8) {
                        load_light(rdram, state, addr, slot);
                    } else {
                        panic!(
                            "G_MOVEMEM G_MV_LIGHT offset/8 {ofs_div8:#04x} is not a public LookAt or light destination"
                        );
                    }
                } else if index == G_MV_MATRIX {
                    assert_eq!(
                        ofs_div8, 0,
                        "G_MOVEMEM G_MV_MATRIX destination offset must be zero"
                    );
                    assert_eq!(
                        length_div8_minus_one, 7,
                        "G_MOVEMEM G_MV_MATRIX must carry one 64-byte Mtx"
                    );
                    let addr = resolve_addr(&state.segments, w1);
                    state.pending_forced_mvp = Some(read_mtx(rdram, addr).unwrap_or_else(|| {
                        panic!("G_MOVEMEM G_MV_MATRIX reads past RDRAM: source={addr:#x}, bytes=64")
                    }));
                } else {
                    crate::render_unsupported_panic(
                        "render.gbi.geometry.movemem",
                        format!(
                            "G_MOVEMEM unsupported index {index:#04x} offset/8 {ofs_div8:#04x}: w0={w0:#010x} w1={w1:#010x}"
                        ),
                    );
                }
            }
            G_GEOMETRYMODE => {
                // F3DEX2 gsSPGeometryMode (§2.4): one atomic clear+set --
                // `mode = (mode & field(w0,0,24)) | w1`, where the w0 low 24
                // bits are the (already-inverted) AND mask. We honor the
                // CULL_FRONT/CULL_BACK bits per-triangle (see cull_mode_from)
                // and the G_LIGHTING bit at G_VTX time (cn = normal -> lit
                // color, see load_vertices). G_FOG replaces vertex alpha from
                // projected depth; shade-smooth remains incomplete.
                let and_mask = w0 & 0x00FF_FFFF;
                state.geometry_mode = (state.geometry_mode & and_mask) | w1;
            }
            G_SETCOMBINE => {
                // Public gbi.h GCCc*w* packing macros (lines 3543-3565)
                // distribute both cycles' RGB/alpha A/B/C/D selectors across
                // w0/w1. `CombinerMode::decode` resolves those raw selectors
                // to semantic sources using the position-specific mux tables.
                state.combiner.mode = CombinerMode::decode(w0, w1);
            }
            G_SETPRIMCOLOR => {
                // gDPSetPrimColor (gbi.h:3672-3682): w0 low byte is the
                // primitive LOD fraction, the preceding byte is the minimum
                // LOD clamp, and w1 is RGBA8888.
                state.combiner.min_lod_level = ((w0 >> 8) & 0xff) as u8;
                state.combiner.prim_lod_fraction = (w0 & 0xff) as u8;
                state.combiner.primitive = w1.to_be_bytes();
            }
            G_SETENVCOLOR => {
                // gDPSetEnvColor -> DPRGBColor (gbi.h:3626-3644): w1 packs
                // RGBA in bits 31..0, one byte per component.
                state.combiner.environment = w1.to_be_bytes();
            }
            G_SETSCISSOR => {
                // SGI RDP Command Summary Table 27: all four edges are
                // unsigned 12-bit quarter-pixels; w1 bits 25/24 enable field
                // scissoring and select the odd field (zero keeps even).
                // The lower-right edge is exclusive: OoT PreRender.c:137
                // passes `lrx + 1` / `lry + 1` when converting its inclusive stored bounds.
                // RT64 likewise stores the fixed rect (rt64_rdp.cpp:974-980)
                // and intersects triangle bounds with it
                // (rt64_rsp.cpp:1140-1154).
                state.scissor = Some(ScissorRect {
                    ulx: ((w0 >> 12) & 0x0FFF) as f32 / 4.0,
                    uly: (w0 & 0x0FFF) as f32 / 4.0,
                    lrx: ((w1 >> 12) & 0x0FFF) as f32 / 4.0,
                    lry: (w1 & 0x0FFF) as f32 / 4.0,
                    field: w1 & (1 << 25) != 0,
                    keep_odd: w1 & (1 << 24) != 0,
                });
            }
            G_TEXRECT | G_TEXRECTFLIP => {
                let (coords, gradients, continuation_bytes) =
                    decode_texture_rectangle_continuation(rdram, pc, *family, raw_rdp, opcode);
                pc += continuation_bytes;
                if continuation_bytes == 16 {
                    state.cmds_decoded += 2;
                    assert!(
                        state.cmds_decoded <= MAX_DL_COMMANDS,
                        "{} display list exceeded the {MAX_DL_COMMANDS}-command budget in the {} continuation at RDRAM {command_pc:#010x}",
                        family.name(),
                        opcode_name(opcode)
                    );
                }
                let tile = ((w1 >> 24) & 0x07) as u8;
                let storage = state.tex.tmem.clone();
                state.ops.push(RenderOp::TextureRectangle(TextureRectangle {
                    ulx: ((w1 >> 12) & 0x0fff) as f32 / 4.0,
                    uly: (w1 & 0x0fff) as f32 / 4.0,
                    lrx: ((w0 >> 12) & 0x0fff) as f32 / 4.0,
                    lry: (w0 & 0x0fff) as f32 / 4.0,
                    tile,
                    s: ((coords >> 16) as u16 as i16) as f32 / 32.0,
                    t: (coords as u16 as i16) as f32 / 32.0,
                    dsdx: (gradients >> 16) as u16 as i16,
                    dtdy: gradients as u16 as i16,
                    flip: opcode == G_TEXRECTFLIP,
                    other_mode: state.other_mode,
                    combiner: state.combiner,
                    blender: active_blender(state),
                    scissor: state.scissor,
                    texture: bind_texture_set(&state.tex, tile, 0, state.other_mode.texture_lut()),
                    texture1: texture_for_tile(
                        &state.tex,
                        tile.wrapping_add(1) & 7,
                        state.other_mode.texture_lut(),
                        &storage,
                    ),
                }));
            }
            G_SPECIAL_1 | G_SPECIAL_2 | G_SPECIAL_3 => panic!(
                "reserved {} command {} at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                family.name(),
                opcode_name(opcode),
                pc - 8
            ),
            G_ENDDL => break,
            _ => crate::render_unsupported_panic(
                "render.gbi.geometry.command",
                format!(
                    "unsupported {} command {} ({opcode:#04x}) at RDRAM {:#010x}: w0={w0:#010x} w1={w1:#010x}",
                    family.name(),
                    opcode_name(opcode),
                    pc - 8
                ),
            ),
        }
    }
}

/// Derive the per-triangle [`CullMode`] from the current F3DEX2 geometry
/// mode's `G_CULL_FRONT`/`G_CULL_BACK` bits (`F3DEX2-CONCEPTS.md` §2.4).
fn cull_mode_from(geometry_mode: u32) -> CullMode {
    let front = geometry_mode & G_CULL_FRONT != 0;
    let back = geometry_mode & G_CULL_BACK != 0;
    match (front, back) {
        (true, true) => CullMode::Both,
        (true, false) => CullMode::Front,
        (false, true) => CullMode::Back,
        (false, false) => CullMode::None,
    }
}

/// Apply one F3DEX2 partial other-mode update. Returns `None` only for a
/// malformed range that cannot fit in a 32-bit H/L word.
fn update_other_mode_word(current: u32, w0: u32, data: u32) -> Option<u32> {
    let length = (w0 & 0xff) + 1;
    let encoded_shift = (w0 >> 8) & 0xff;
    let shift = 32u32.checked_sub(encoded_shift.checked_add(length)?)?;
    if length > 32 {
        return None;
    }
    let mask = if length == 32 {
        u32::MAX
    } else {
        (((1u64 << length) - 1) << shift) as u32
    };
    // Deliberately OR the complete data word, as RT64 does. Public gbi.h's
    // predefined render modes include G_AC_DITHER in bits 0..1 even though
    // gDPSetRenderMode requests the nominal bits-3..31 range
    // (`gbi.h:700-702,756-758,802-804,824-827,3484-3487`). Masking `data`
    // here would erase that alpha-compare mode from real OoT display lists.
    Some((current & !mask) | data)
}

/// The texture to bind to triangles emitted right now. `None` means
/// `G_TEXTURE` disabled texturing; enabling it without a live TMEM image is a
/// named failure rather than a white/flat-shaded substitution.
fn texture_for_tile(
    tex: &TexState,
    tile_index: u8,
    texture_lut: u8,
    storage: &std::rc::Rc<Tmem>,
) -> Option<Texture> {
    let index = usize::from(tile_index);
    let tile = tex.tiles[index];
    if !tex.tmem.is_initialized() {
        return None;
    }
    // Programming Manual Chapter 13, "Tile Attributes": LRS/LRT are used
    // only for clamping. A wrapped axis is valid even when its unsigned
    // origin sits near 1024 and its unused upper clamp bound is numerically
    // lower; its address domain comes from the mask instead.
    let axis_dimension = |low: u16, high: u16, clamp: bool, mask: u8| {
        if mask != 0 && !clamp {
            Some(1_u32 << mask)
        } else {
            let low = low / 4;
            let high = high / 4;
            (high >= low).then(|| u32::from(high - low + 1))
        }
    };
    let width = axis_dimension(tile.uls, tile.lrs, tile.clamp_s, tile.mask_s)?;
    let height = axis_dimension(tile.ult, tile.lrt, tile.clamp_t, tile.mask_t)?;
    if width > 1024 || height > 1024 {
        return None;
    }
    Some(Texture {
        format: tile.fmt,
        size: tile.siz,
        width,
        height,
        texels: std::rc::Rc::new(Vec::new()),
        clamp_s: tile.clamp_s,
        clamp_t: tile.clamp_t,
        mirror_s: tile.mirror_s,
        mirror_t: tile.mirror_t,
        mask_s: tile.mask_s,
        mask_t: tile.mask_t,
        shift_s: tile.shift_s,
        shift_t: tile.shift_t,
        origin_s: tile.uls as f32 / 4.0,
        origin_t: tile.ult as f32 / 4.0,
        tmem: Some(std::rc::Rc::new(TmemTexture {
            storage: storage.clone(),
            tile,
            texture_lut,
        })),
        lod: None,
    })
}

fn bind_texture_set(
    tex: &TexState,
    primitive_tile: u8,
    max_level: u8,
    texture_lut: u8,
) -> Option<Texture> {
    let storage = tex.tmem.clone();
    let tiles =
        std::array::from_fn(|tile| texture_for_tile(tex, tile as u8, texture_lut, &storage));
    Some(
        tiles[usize::from(primitive_tile)]
            .clone()?
            .with_lod_snapshot(tiles, primitive_tile, max_level),
    )
}

fn active_texture(tex: &TexState, other_mode: OtherMode) -> Option<Texture> {
    if tex.tex_enabled {
        Some(
            bind_texture_set(
                tex,
                tex.tex_tile,
                tex.tex_max_level,
                other_mode.texture_lut(),
            )
            .unwrap_or_else(|| {
                let tile = tex.tiles[usize::from(tex.tex_tile)];
                panic!(
                    "G_TEXTURE enables tile {} but no initialized TMEM image with a valid G_SETTILESIZE extent exists: ({}, {})..({}, {})",
                    tex.tex_tile, tile.uls, tile.ult, tile.lrs, tile.lrt
                )
            }),
        )
    } else {
        None
    }
}

fn active_blender(state: &DecodeState) -> BlenderState {
    BlenderState::from_other_mode(
        state.other_mode.raw_low(),
        state.other_mode.raw_high(),
        state.blend_color,
        state.fog_color,
    )
}

/// Recompute the cached model-view-projection matrix from the current stack.
///
/// `state.proj` already holds the accumulated `view * proj` product (built
/// left-multiplied in the G_MTX handler, hardware order). The full transform
/// is `mvp = modelview * (view * proj)` = `M * V * P`, kept in hardware
/// `[row][col]` layout. The incoming vertex is applied by `transform_point`
/// as a ROW vector (`clip = v_row · mvp`), reproducing the hardware's
/// `v · M · V · P` with a sane `w` (`≈ -z_eye`, the perspective depth). See
/// `transform_point` for why applying it as a column vector (`mvp · v`)
/// instead is the transpose and produces the sign-flipping ±thousands `w`.
fn recompute_mvp(state: &mut DecodeState) {
    // An ordinary matrix-stack operation supersedes both halves of any
    // force-matrix override and rebuilds the concatenated transform from the
    // public modelview/projection stacks.
    state.pending_forced_mvp = None;
    // A missing projection stack entry means identity projection, not "skip
    // the already-loaded modelview". This function is called only after an
    // actual matrix-stack operation, so the raw-coordinate fixture convention
    // still keeps `mvp == None` until the first G_MTX.
    state.mvp = Some(match state.proj {
        Some(p) => mat_mul(&state.modelview, &p),
        None => state.modelview,
    });
}

/// Load `n` vertices starting at cache slot `v0` from the (segmented) array
/// at `arr_addr`, applying the active transform if one is loaded.
fn load_vertices(
    rdram: &[u8],
    state: &mut DecodeState,
    arr_addr: u32,
    n: usize,
    v0: usize,
    family: GeometryWireFamily,
) {
    if matches!(
        family,
        GeometryWireFamily::F3dlx | GeometryWireFamily::F3dlxRej | GeometryWireFamily::F3dlx2Rej
    ) && state.mvp.is_some()
    {
        crate::render_unsupported_panic(
            "render.gbi.geometry.pixel-precision",
            format!(
                "{} transformed G_VTX requires exact pixel-precision rounding that the public manuals do not specify",
                family.name()
            ),
        );
    }
    let base = resolve_addr(&state.segments, arr_addr);
    assert!(
        n > 0
            && v0
                .checked_add(n)
                .is_some_and(|end| end <= state.vtx_cache.len()),
        "G_VTX destination range {v0}..{} is outside cache slots 0..={} or empty",
        v0.saturating_add(n),
        state.vtx_cache.len() - 1
    );
    let byte_len = n
        .checked_mul(VTX_STRIDE)
        .unwrap_or_else(|| panic!("G_VTX count {n} overflows the host address space"));
    let source_end = base.checked_add(byte_len).unwrap_or_else(|| {
        panic!("G_VTX source {base:#x} plus {byte_len} bytes overflows the host address space")
    });
    assert!(
        source_end <= rdram.len(),
        "G_VTX reads past RDRAM: source={base:#x}, count={n}, bytes={byte_len}, rdram_bytes={}",
        rdram.len()
    );
    for i in 0..n {
        let off = base + i * VTX_STRIDE;
        // Swizzled reads (recomp MEM_H / MEM_BU): vertex arrays are DMA'd
        // from ROM through the `^3` per-byte swizzle, same as the DL words.
        let x = read_i16(rdram, off) as f32;
        let y = read_i16(rdram, off + 2) as f32;
        let z = read_i16(rdram, off + 4) as f32;
        // tc[2] (offsets 8, 10): raw S/T in S10.5 fixed-point (§2.1). Scale
        // by the active G_TEXTURE S/T scale, then convert S10.5 -> texels
        // (÷32). The result is texels the rasterizer addresses directly.
        let raw_s = read_i16(rdram, off + 8) as f32;
        let raw_t = read_i16(rdram, off + 10) as f32;
        // cn[4] at offsets 12..16. The alpha byte is always alpha. The RGB
        // bytes are EITHER a flat vertex color (G_LIGHTING off) OR a signed
        // s8 NORMAL (G_LIGHTING on) that must be LIT into a color -- reading
        // a normal as a color is what produced the "rainbow fan" (signed
        // normal components read as unsigned channels). See G_LIGHTING.
        let source_alpha = read_u8(rdram, off + 15);
        let uses_normal = state.geometry_mode & (G_LIGHTING | G_TEXTURE_GEN) != 0;
        let normal = uses_normal.then(|| {
            [
                (read_u8(rdram, off + 12) as i8) as f32 / 127.0,
                (read_u8(rdram, off + 13) as i8) as f32 / 127.0,
                (read_u8(rdram, off + 14) as i8) as f32 / 127.0,
            ]
        });
        let (r, g, b) = if state.geometry_mode & G_LIGHTING != 0 {
            let [lr, lg, lb] = light_vertex(state, normal.expect("lighting normal missing"));
            (lr, lg, lb)
        } else {
            (
                read_u8(rdram, off + 12),
                read_u8(rdram, off + 13),
                read_u8(rdram, off + 14),
            )
        };
        let (s, t) = if state.geometry_mode & G_TEXTURE_GEN != 0 {
            generated_texture_coords(state, normal.expect("texture-generation normal missing"))
        } else {
            (
                raw_s * state.tex.tex_scale_s / 32.0,
                raw_t * state.tex.tex_scale_t / 32.0,
            )
        };

        let (sx, sy, sz, sw, z_screen, clip_code, ndc_z, clip_position) =
            project_vertex(state, x, y, z);
        let a = if state.geometry_mode & G_FOG != 0 {
            fog_alpha(state.fog, ndc_z)
        } else {
            source_alpha
        };
        #[cfg(not(test))]
        {
            projdump::note_pz(sz);
            // On-screen NDC test: perspective-divide the clip coords and check
            // the NDC cube [-1,1]^3 (with a positive-w gate: w<=0 is behind cam).
            let onscreen = if sw > 1e-4 {
                let nx = sx; // sx/sy are already viewport-mapped pixels below;
                let _ = nx;
                // Reconstruct NDC from clip via mvp to classify honestly:
                false
            } else {
                false
            };
            let _ = onscreen;
            if let Some(mvp) = state.mvp {
                let clip = transform_point(&mvp, x, y, z);
                let (cx, cy, cz, cw) = (clip[0], clip[1], clip[2], clip[3]);
                let inside = cw.abs() > 1e-4
                    && (clip[0] / cw).abs() <= 1.0
                    && (clip[1] / cw).abs() <= 1.0
                    && (clip[2] / cw).abs() <= 1.0
                    && cw > 0.0;
                projdump::note_w(cw, inside);
                if projdump::should_log_vtx() {
                    eprintln!(
                        "[FN64_DUMP_PROJ] vtx ob=({x:.0},{y:.0},{z:.0}) -> clip=({cx:.2},{cy:.2},{cz:.2},w={cw:.4}) ndc=({:.3},{:.3},{:.3}) inside_cube={inside}",
                        cx / cw,
                        cy / cw,
                        cz / cw
                    );
                }
            }
        }
        state.vtx_cache[v0 + i] = Vertex {
            x: sx,
            y: sy,
            z: sz,
            r,
            g,
            b,
            a,
            s,
            t,
            w: sw,
            z_screen,
            clip_code,
            clip_position,
        };
    }
}

/// Map a model-space vertex to screen space. If a full projection*modelview
/// is active, apply it, perspective-divide, and map NDC [-1,1] through the
/// explicitly loaded viewport. A matrix without `G_MV_VIEWPORT` is invalid
/// input to this reference path and traps instead of inventing screen state.
/// If NO transform is loaded at all, the raw `ob` x/y are already screen
/// coordinates (the pre-existing reference-fixture convention) and pass
/// through unchanged.
fn project_vertex(
    state: &DecodeState,
    x: f32,
    y: f32,
    z: f32,
) -> (f32, f32, f32, f32, u32, u8, f32, Option<[f32; 4]>) {
    if state.persp_normalize.rejects_geometry() {
        // Public `.16` scale zero collapses both transformed coordinates and
        // W before the limited-precision divide. Retain nonpositive W so every
        // primitive path rejects the degenerate result instead of inventing a
        // finite host-float quotient.
        return (0.0, 0.0, 0.0, 0.0, 0, 0, 0.0, Some([0.0; 4]));
    }
    match state.mvp {
        Some(mvp) => {
            let clip = transform_point(&mvp, x, y, z);
            let clip_code = homogeneous_clip_code(clip);
            // Keep the true clip-space w for near-plane culling (a vertex with
            // w <= 0 is at/behind the camera). Guard only the DIVIDE against a
            // near-zero w so the perspective divide doesn't overflow; the
            // decision to draw is made from the un-guarded `clip[3]` (returned
            // as the 4th component) in resolve_tri.
            let true_w = clip[3];
            let w = if true_w.abs() > 1e-6 { true_w } else { 1e-6 };
            let ndc_x = clip[0] / w;
            let ndc_y = clip[1] / w;
            let ndc_z = clip[2] / w;
            let vp = state.viewport.as_ref().expect(
                "G_VTX with an active matrix requires G_MOVEMEM G_MV_VIEWPORT before transformed vertices",
            );
            // vscale/vtrans are in pixels (already /4 in read_viewport).
            let px = ndc_x * vp.sx + vp.tx;
            // N64 screen Y is top-down; NDC +Y is up, so flip.
            let py = -ndc_y * vp.sy + vp.ty;
            let pz = ndc_z * vp.sz + vp.tz;
            (
                px,
                py,
                pz,
                true_w,
                screen_depth_to_fixed(pz),
                clip_code,
                ndc_z,
                Some(clip),
            )
        }
        None => {
            // No transform: raw screen coords (reference-fixture path). w=1 so
            // the near-plane cull never rejects the raw/fixture geometry.
            (x, y, 0.0, 1.0, 0, 0, 0.0, None)
        }
    }
}

fn fog_alpha(fog: FogFactor, ndc_z: f32) -> u8 {
    (ndc_z * f32::from(fog.multiplier) + f32::from(fog.offset)).clamp(0.0, 255.0) as u8
}

/// Derive the six clipping-code bits retained by the F3DEX2 vertex cache.
/// Public `gSPCullDisplayList` documentation specifies that volume culling
/// intersects these per-vertex codes and is independent of `gSPClipRatio`.
fn homogeneous_clip_code([x, y, z, w]: [f32; 4]) -> u8 {
    let mut code = 0;
    if x < -w {
        code |= CLIP_NEG_X;
    }
    if x > w {
        code |= CLIP_POS_X;
    }
    if y < -w {
        code |= CLIP_NEG_Y;
    }
    if y > w {
        code |= CLIP_POS_Y;
    }
    if z < -w {
        code |= CLIP_NEG_Z;
    }
    if z > w {
        code |= CLIP_POS_Z;
    }
    code
}

fn screen_depth_to_fixed(z: f32) -> u32 {
    if !z.is_finite() || z <= 0.0 {
        0
    } else if z >= u32::MAX as f32 / 65536.0 {
        u32::MAX
    } else {
        (z * 65536.0) as u32
    }
}

/// Extract the three F3DEX2 triangle vertex-cache slot indices from a
/// command word: three 7-bit fields at bit offsets 17, 9, 1 (F3DEX2-
/// CONCEPTS.md §2.2). Each field is already the slot (0-31).
fn tri_indices(w: u32) -> [u32; 3] {
    [(w >> 17) & 0x7F, (w >> 9) & 0x7F, (w >> 1) & 0x7F]
}

/// A vertex is at/behind the near plane when its clip-space `w` is not
/// positive. Projecting such a vertex divides by a non-positive number and
/// flings it across the screen; a triangle touching one is dropped.
#[inline]
fn behind_near_plane(v: &Vertex) -> bool {
    v.w <= 1e-4
}

fn resolve_tri(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    resolve_tri_with_admission(
        vtx_cache,
        idx,
        vtx_cache.len(),
        TriangleAdmission::ClipNear,
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    )
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TriangleAdmission {
    ClipNear,
    Unclipped,
    RejectBox(ClipRatio),
}

fn family_triangle_admission(
    family: GeometryWireFamily,
    geometry_mode: u32,
    clip_ratio: ClipRatio,
) -> TriangleAdmission {
    match family {
        GeometryWireFamily::F3dlx if geometry_mode & LEGACY_G_CLIPPING == 0 => {
            TriangleAdmission::Unclipped
        }
        GeometryWireFamily::F3dex2NoN => TriangleAdmission::Unclipped,
        family if family.is_reject() => TriangleAdmission::RejectBox(clip_ratio),
        _ => TriangleAdmission::ClipNear,
    }
}

fn vertex_inside_reject_box(vertex: &Vertex, ratio: ClipRatio) -> bool {
    let Some([x, y, z, w]) = vertex.clip_position else {
        // The raw-coordinate path exists only for deterministic renderer
        // fixtures and has no RSP transform from which a reject box can be
        // reconstructed. Its coordinates are already screen-space input.
        return true;
    };
    x >= -f32::from(ratio.neg_x) * w
        && x <= f32::from(ratio.pos_x) * w
        && y >= -f32::from(ratio.neg_y) * w
        && y <= f32::from(ratio.pos_y) * w
        // The public F3DLX.Rej contract rejects against the far plane but
        // deliberately has no near-plane reject.
        && z <= w
}

#[allow(clippy::too_many_arguments)]
fn resolve_tri_for_family(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    family: GeometryWireFamily,
    geometry_mode: u32,
    clip_ratio: ClipRatio,
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    resolve_tri_with_admission(
        vtx_cache,
        idx,
        family.cache_capacity(),
        family_triangle_admission(family, geometry_mode, clip_ratio),
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_tri_with_admission(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    cache_capacity: usize,
    admission: TriangleAdmission,
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    assert!(
        idx.iter().all(|&i| (i as usize) < cache_capacity),
        "G_TRI vertex-cache slots {idx:?} must all be within 0..={}",
        cache_capacity - 1
    );
    let v = [
        vtx_cache[idx[0] as usize],
        vtx_cache[idx[1] as usize],
        vtx_cache[idx[2] as usize],
    ];
    match admission {
        TriangleAdmission::ClipNear if v.iter().any(behind_near_plane) => return None,
        TriangleAdmission::RejectBox(ratio)
            if v.iter()
                .any(|vertex| !vertex_inside_reject_box(vertex, ratio)) =>
        {
            return None;
        }
        TriangleAdmission::ClipNear
        | TriangleAdmission::Unclipped
        | TriangleAdmission::RejectBox(_) => {}
    }
    Some(Triangle {
        v: [
            vtx_cache[idx[0] as usize],
            vtx_cache[idx[1] as usize],
            vtx_cache[idx[2] as usize],
        ],
        scissor: None,
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    })
}

fn resolve_line(
    vtx_cache: &[Vertex],
    slots: [usize; 2],
    width_parameter: u8,
    snapshot: LineDecodeSnapshot,
) -> Option<Line> {
    let [start, end] = slots;
    let Some((&start_vertex, &end_vertex)) = vtx_cache.get(start).zip(vtx_cache.get(end)) else {
        panic!("G_LINE3D cache slots {start} and {end} must both be within F3DEX2 slots 0..=31");
    };
    let [start_vertex, end_vertex] = clip_line_to_homogeneous_volume(
        start_vertex,
        end_vertex,
        snapshot.viewport,
        snapshot.clip_ratio,
    )?;
    Some(Line {
        v: [start_vertex, end_vertex],
        width: 1.5 + f32::from(width_parameter) * 0.5,
        smooth_shading: snapshot.smooth_shading,
        scissor: snapshot.scissor,
        texture: snapshot.texture,
        other_mode: snapshot.other_mode,
        combiner: snapshot.combiner,
        blender: snapshot.blender,
    })
}

fn interpolate_line_vertex(start: Vertex, end: Vertex, parameter: f32) -> Vertex {
    let interpolate = |a: f32, b: f32| a + (b - a) * parameter;
    let channel = |a: u8, b: u8| {
        interpolate(f32::from(a), f32::from(b))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Vertex {
        x: interpolate(start.x, end.x),
        y: interpolate(start.y, end.y),
        z: interpolate(start.z, end.z),
        r: channel(start.r, end.r),
        g: channel(start.g, end.g),
        b: channel(start.b, end.b),
        a: channel(start.a, end.a),
        s: interpolate(start.s, end.s),
        t: interpolate(start.t, end.t),
        w: interpolate(start.w, end.w),
        z_screen: 0,
        clip_code: 0,
        clip_position: None,
    }
}

fn project_clipped_line_vertex(
    mut vertex: Vertex,
    clip: [f32; 4],
    viewport: Option<Viewport>,
) -> Vertex {
    let reciprocal_w = 1.0 / clip[3];
    let ndc = [
        clip[0] * reciprocal_w,
        clip[1] * reciprocal_w,
        clip[2] * reciprocal_w,
    ];
    let viewport = viewport
        .expect("clipped transformed G_LINE3D requires the G_MV_VIEWPORT state used by G_VTX");
    vertex.x = ndc[0] * viewport.sx + viewport.tx;
    vertex.y = -ndc[1] * viewport.sy + viewport.ty;
    vertex.z = ndc[2] * viewport.sz + viewport.tz;
    vertex.w = clip[3];
    vertex.z_screen = screen_depth_to_fixed(vertex.z);
    vertex.clip_code = homogeneous_clip_code(clip);
    vertex.clip_position = Some(clip);
    vertex
}

fn clip_line_to_homogeneous_volume(
    mut start: Vertex,
    mut end: Vertex,
    viewport: Option<Viewport>,
    clip_ratio: ClipRatio,
) -> Option<[Vertex; 2]> {
    let (Some(mut start_clip), Some(mut end_clip)) = (start.clip_position, end.clip_position)
    else {
        if behind_near_plane(&start) || behind_near_plane(&end) {
            crate::render_unsupported_panic(
                "render.gbi.line.modified-position-clipping",
                "G_LINE3D cannot reconstruct homogeneous clipping after G_MODIFYVTX screen-position writes",
            );
        }
        return Some([start, end]);
    };
    let plane_distance = |clip: [f32; 4], plane: usize| match plane {
        0 => f32::from(clip_ratio.neg_x) * clip[3] + clip[0],
        1 => f32::from(clip_ratio.pos_x) * clip[3] - clip[0],
        2 => f32::from(clip_ratio.neg_y) * clip[3] + clip[1],
        3 => f32::from(clip_ratio.pos_y) * clip[3] - clip[1],
        4 => clip[3] + clip[2],
        5 => clip[3] - clip[2],
        _ => unreachable!(),
    };
    for plane in 0..6 {
        let start_distance = plane_distance(start_clip, plane);
        let end_distance = plane_distance(end_clip, plane);
        if start_distance < 0.0 && end_distance < 0.0 {
            return None;
        }
        if (start_distance < 0.0) != (end_distance < 0.0) {
            let parameter = start_distance / (start_distance - end_distance);
            let clip = std::array::from_fn(|component| {
                start_clip[component] + (end_clip[component] - start_clip[component]) * parameter
            });
            let vertex = interpolate_line_vertex(start, end, parameter);
            if start_distance < 0.0 {
                start = vertex;
                start_clip = clip;
            } else {
                end = vertex;
                end_clip = clip;
            }
        }
    }
    if start_clip[3] <= 1e-6 || end_clip[3] <= 1e-6 {
        return None;
    }
    Some([
        project_clipped_line_vertex(start, start_clip, viewport),
        project_clipped_line_vertex(end, end_clip, viewport),
    ])
}

/// Compatibility declaration for fixture/simple and raw-RDP modes. Geometry
/// mode reports the exact families represented by its digest catalog instead.
pub const SUPPORTED: &[UcodeId] = &[UcodeId::F3dex2];

#[cfg(test)]
// Wire-encoding tests intentionally spell zero-valued bitfields, fixed 4x4
// indices, and traced f32 literals in their source form so the evidence stays
// directly comparable to the cited command/matrix layouts.
#[allow(
    clippy::excessive_precision,
    clippy::identity_op,
    clippy::needless_range_loop
)]
mod tests {
    use super::*;

    fn set_convert_command(coefficients: [i16; 6]) -> (u32, u32) {
        let field = |value: i16| u32::from(value as u16) & 0x1ff;
        let [k0, k1, k2, k3, k4, k5] = coefficients.map(field);
        (
            ((G_SETCONVERT as u32) << 24) | (k0 << 13) | (k1 << 4) | ((k2 >> 5) & 0x0f),
            ((k2 & 0x1f) << 27) | (k3 << 18) | (k4 << 9) | k5,
        )
    }

    /// Write a logical big-endian s16 at `off` through the recomp `^3` byte
    /// swizzle (mirrors the decoder's `read_i16`/`read_u16` memory model).
    fn wr_i16(rdram: &mut [u8], off: usize, v: i16) {
        let b = (v as u16).to_be_bytes();
        rdram[off ^ 3] = b[0];
        rdram[(off + 1) ^ 3] = b[1];
    }

    fn centered_viewport() -> Viewport {
        Viewport {
            sx: 160.0,
            sy: 120.0,
            sz: 127.75,
            tx: 160.0,
            ty: 120.0,
            tz: 127.75,
        }
    }

    fn wr_centered_viewport(rdram: &mut [u8], off: usize) {
        for (index, value) in [640, 480, 511, 0, 640, 480, 511, 0].into_iter().enumerate() {
            wr_i16(rdram, off + index * 2, value);
        }
    }

    fn movemem_viewport_word() -> u32 {
        ((G_MOVEMEM as u32) << 24) | (1 << 19) | u32::from(G_MV_VIEWPORT)
    }

    /// Write an aligned logical 32-bit word (recomp `MEM_W`: native-endian,
    /// no swizzle), matching the decoder's `read_u32`. Used to plant raw
    /// display-list command words.
    fn wr_u32(rdram: &mut [u8], off: usize, v: u32) {
        rdram[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }

    /// Plant one 8-byte F3DEX2 command (`w0`, `w1`) at byte offset `off`.
    fn wr_cmd(rdram: &mut [u8], off: usize, w0: u32, w1: u32) {
        wr_u32(rdram, off, w0);
        wr_u32(rdram, off + 4, w1);
    }

    fn dma_io_word(write_to_dram: bool, rsp_address: u16, size: u16) -> u32 {
        assert!(rsp_address < 0x2000 && rsp_address.is_multiple_of(8));
        assert!((1..=0x1000).contains(&size));
        ((G_DMA_IO as u32) << 24)
            | (u32::from(write_to_dram) << 23)
            | ((u32::from(rsp_address) / 8) << 13)
            | (u32::from(size) - 1)
    }

    fn load_ucode_word(data_size: u16) -> u32 {
        assert!((1..=0x1000).contains(&data_size));
        ((G_LOAD_UCODE as u32) << 24) | (u32::from(data_size) - 1)
    }

    #[test]
    fn simple_decoder_rejects_unknown_opcode_with_wire_context() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut rdram = vec![0u8; 16];
        rdram[..4].copy_from_slice(&0x7f12_3456u32.to_be_bytes());
        rdram[4..8].copy_from_slice(&0x89ab_cdefu32.to_be_bytes());
        rdram[8..12].copy_from_slice(&((G_ENDDL as u32) << 24).to_be_bytes());

        let error = decode_display_list(&rdram, 0).unwrap_err().to_string();

        assert!(error.contains("reference-simple"));
        assert!(error.contains("opcode 0x7f"));
        assert!(error.contains("w0=0x7f123456"));
        assert!(error.contains("w1=0x89abcdef"));
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Render
        );
        assert_eq!(events[0].operation, "render.gbi.simple.command");
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert_eq!(events[0].guest_cycle, None);
    }

    #[test]
    fn simple_decoder_rejects_truncation_without_end_command() {
        let error = decode_display_list(&[0; 4], 0).unwrap_err().to_string();
        assert!(error.contains("truncated command"));
        assert!(error.contains("without G_ENDDL"));
    }

    #[test]
    fn simple_decoder_rejects_out_of_bounds_vertex_range() {
        let mut rdram = vec![0u8; 16];
        let w0 = ((G_VTX as u32) << 24) | (1 << 12);
        rdram[..4].copy_from_slice(&w0.to_be_bytes());
        rdram[4..8].copy_from_slice(&0x100u32.to_be_bytes());
        rdram[8..12].copy_from_slice(&((G_ENDDL as u32) << 24).to_be_bytes());

        let error = decode_display_list(&rdram, 0).unwrap_err().to_string();
        assert!(error.contains("G_VTX"));
        assert!(error.contains("RDRAM=0x00000100"));
    }

    #[test]
    fn command_trace_follows_segmented_calls_and_fingerprints_targets() {
        let mut rdram = vec![0u8; 0x4000];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_SEGMENT as u32) << 16) | 0x0c,
            0x8000_3000,
        );
        wr_cmd(&mut rdram, 0x1008, (G_DL as u32) << 24, 0x0300_0100);
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
        wr_cmd(
            &mut rdram,
            0x3100,
            ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
            0x0300_0200,
        );
        wr_cmd(&mut rdram, 0x3108, (G_ENDDL as u32) << 24, 0);
        rdram[0x3200] = 0x5a;

        let trace = trace_display_list_f3dex2(&rdram, 0x1000);
        assert!(trace.contains("SEG depth=0 segment=3 base=0x003000"));
        assert!(trace.contains("ENTER depth=1 segmented=0x03000100 resolved=0x003100"));
        assert!(trace.contains("target=0x003200 bytes=16 nonzero=1"));
        assert!(trace.contains("SUMMARY commands=5"));
        assert!(trace.contains("OPCODE op=0xdf count=2"));
    }

    #[test]
    fn dma_io_round_trips_logical_rdram_bytes_through_persistent_imem() {
        const DL: usize = 0x1000;
        const SOURCE: usize = 0x1800;
        const DESTINATION: usize = 0x1900;
        const IMEM_ADDRESS: u16 = 0x1040;
        let expected = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0];
        let mut rdram = vec![0u8; 0x2000];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(SOURCE as u32),
            &expected,
        );
        wr_cmd(
            &mut rdram,
            DL,
            dma_io_word(false, IMEM_ADDRESS, expected.len() as u16),
            SOURCE as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 8,
            dma_io_word(true, IMEM_ADDRESS, expected.len() as u16),
            DESTINATION as u32,
        );
        wr_cmd(&mut rdram, DL + 16, (G_ENDDL as u32) << 24, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        execute_display_list_f3dex2_ops(&mut rdram, &mut rsp_memory, DL as u32).unwrap();

        let rsp_address = fn64_runtime::RspMemAddr::from_register(u32::from(IMEM_ADDRESS));
        assert_eq!(rsp_memory.read_bytes(rsp_address, 8).unwrap(), expected);
        assert_eq!(rsp_memory.imem_generation(), 1);
        let mut round_trip = [0; 8];
        fn64_runtime::RdramView::from_storage(&rdram).copy_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(DESTINATION as u32),
            &mut round_trip,
        );
        assert_eq!(round_trip, expected);
    }

    #[test]
    fn dma_io_write_is_visible_to_the_next_command_in_the_same_stream() {
        const DL: usize = 0x1000;
        const DMEM_ADDRESS: u16 = 0x0080;
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(
            &mut rdram,
            DL,
            dma_io_word(true, DMEM_ADDRESS, 8),
            (DL + 8) as u32,
        );
        wr_cmd(&mut rdram, DL + 8, (G_SPECIAL_1 as u32) << 24, 0);
        wr_cmd(&mut rdram, DL + 16, (G_SPECIAL_1 as u32) << 24, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_register(u32::from(DMEM_ADDRESS)),
                &[G_ENDDL, 0, 0, 0, 0, 0, 0, 0],
            )
            .unwrap();
        let state =
            execute_display_list_f3dex2_state(&mut rdram, &mut rsp_memory, DL as u32).unwrap();

        assert_eq!(state.cmds_decoded, 2);
        assert_eq!(read_u32(&rdram, DL + 8), (G_ENDDL as u32) << 24);
    }

    #[test]
    fn load_ucode_transfers_data_and_text_before_the_next_command() {
        const DL: usize = 0x1000;
        const TEXT: usize = 0x2000;
        const DATA: usize = 0x3800;
        const ROUND_TRIP: usize = 0x4800;
        const DATA_SIZE: usize = 32;
        let text: Vec<u8> = (0..SP_UCODE_SIZE)
            .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
            .collect();
        let data: Vec<u8> = (0..DATA_SIZE)
            .map(|index| (index as u8).wrapping_mul(13).wrapping_add(7))
            .collect();
        let mut rdram = vec![0u8; 0x5000];
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT as u32), &text);
        view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA as u32), &data);
        wr_cmd(&mut rdram, DL, (G_RDPHALF_1 as u32) << 24, DATA as u32);
        wr_cmd(
            &mut rdram,
            DL + 8,
            load_ucode_word(DATA_SIZE as u16),
            TEXT as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 16,
            dma_io_word(true, 0x1000, 8),
            ROUND_TRIP as u32,
        );
        wr_cmd(&mut rdram, DL + 24, (G_ENDDL as u32) << 24, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        rsp_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(
                    fn64_runtime::RspMemoryBank::Dmem,
                    DATA_SIZE as u16,
                ),
                &[0xa5; 8],
            )
            .unwrap();
        execute_display_list_f3dex2_ops(&mut rdram, &mut rsp_memory, DL as u32).unwrap();

        assert_eq!(
            rsp_memory
                .read_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0,),
                    DATA_SIZE,
                )
                .unwrap(),
            data
        );
        assert_eq!(
            rsp_memory
                .read_bytes(
                    fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        DATA_SIZE as u16,
                    ),
                    8,
                )
                .unwrap(),
            [0xa5; 8],
            "the command replaces only the declared DMEM data section"
        );
        assert_eq!(
            rsp_memory
                .bank(fn64_runtime::RspMemoryBank::Imem)
                .as_slice(),
            text
        );
        assert_eq!(rsp_memory.imem_generation(), 1);
        let mut round_trip = [0; 8];
        fn64_runtime::RdramView::from_storage(&rdram).copy_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(ROUND_TRIP as u32),
            &mut round_trip,
        );
        assert_eq!(round_trip, text[..8]);
    }

    #[test]
    fn f3dex2_load_ucode_preserves_the_display_list_return_stack() {
        const ROOT: usize = 0x1000;
        const CHILD: usize = 0x1100;
        const TEXT: usize = 0x2000;
        const DATA: usize = 0x3000;
        let mut rdram = vec![0u8; 0x4000];
        wr_cmd(&mut rdram, ROOT, (G_DL as u32) << 24, CHILD as u32);
        wr_cmd(&mut rdram, ROOT + 8, (G_NOOP as u32) << 24, 0xfeed_beef);
        wr_cmd(&mut rdram, ROOT + 16, (G_ENDDL as u32) << 24, 0);
        wr_cmd(&mut rdram, CHILD, (G_RDPHALF_1 as u32) << 24, DATA as u32);
        wr_cmd(&mut rdram, CHILD + 8, load_ucode_word(8), TEXT as u32);
        wr_cmd(&mut rdram, CHILD + 16, (G_ENDDL as u32) << 24, 0);

        let state = execute_display_list_f3dex2_state(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            ROOT as u32,
        )
        .unwrap();

        assert_eq!(state.cmds_decoded, 6);
        assert_eq!(state.dl_depth, 0);
    }

    #[test]
    fn self_load_stops_before_decoding_an_unadmitted_microcode_family() {
        const DL: usize = 0x1000;
        const TEXT: usize = 0x2000;
        const DATA: usize = 0x3000;
        let target_text = vec![0x5a; SP_UCODE_SIZE];
        let mut rdram = vec![0u8; 0x4000];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(TEXT as u32),
            &target_text,
        );
        wr_cmd(&mut rdram, DL, (G_RDPHALF_1 as u32) << 24, DATA as u32);
        wr_cmd(&mut rdram, DL + 8, load_ucode_word(8), TEXT as u32);
        wr_cmd(&mut rdram, DL + 16, (G_SPECIAL_1 as u32) << 24, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let mut catalog = F3dex2UcodeCatalog::default();
        catalog.admit_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem));
        let error = execute_display_list_f3dex2_ops_admitted(
            &mut rdram,
            &mut rsp_memory,
            DL as u32,
            &catalog,
        )
        .expect_err("changed unadmitted IMEM must leave the F3DEX2 HLE lane");

        match error {
            RenderError::RequiresLle { ucode_sha256 } => assert_eq!(
                ucode_sha256,
                UcodeDigest::from_text(&target_text).as_bytes()
            ),
            other => panic!("expected typed LLE handoff, got {other}"),
        }
        assert_eq!(
            rsp_memory
                .bank(fn64_runtime::RspMemoryBank::Imem)
                .as_slice(),
            target_text
        );
        assert_eq!(rsp_memory.imem_generation(), 1);
    }

    #[test]
    fn exact_digest_admits_a_compatible_self_load_target() {
        const DL: usize = 0x1000;
        const TEXT: usize = 0x2000;
        const DATA: usize = 0x3000;
        let target_text = vec![0xa6; SP_UCODE_SIZE];
        let mut rdram = vec![0u8; 0x4000];
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(TEXT as u32),
            &target_text,
        );
        wr_cmd(&mut rdram, DL, (G_RDPHALF_1 as u32) << 24, DATA as u32);
        wr_cmd(&mut rdram, DL + 8, load_ucode_word(8), TEXT as u32);
        wr_cmd(&mut rdram, DL + 16, (G_NOOP as u32) << 24, 0xcafe_babe);
        wr_cmd(&mut rdram, DL + 24, (G_ENDDL as u32) << 24, 0);

        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let mut catalog = F3dex2UcodeCatalog::default();
        let admitted = catalog.admit_text(&target_text);
        assert_eq!(admitted, UcodeDigest::from_text(&target_text));
        let operations = execute_display_list_f3dex2_ops_admitted(
            &mut rdram,
            &mut rsp_memory,
            DL as u32,
            &catalog,
        )
        .unwrap();

        assert!(operations.is_empty());
        assert_eq!(rsp_memory.imem_generation(), 1);
    }

    #[test]
    fn public_rdp_tagged_noop_and_rsp_noop_are_intentional_commands() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(&mut rdram, 0x1000, (G_NOOP as u32) << 24, 0xfeed_beef);
        wr_cmd(&mut rdram, 0x1008, (G_SPNOOP as u32) << 24, 0);
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
        assert_eq!(state.cmds_decoded, 3);
        assert!(state.ops.is_empty());
    }

    #[test]
    #[should_panic(expected = "G_SPNOOP reserved second word must be zero")]
    fn rsp_noop_rejects_non_public_payload() {
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(&mut rdram, 0x1000, (G_SPNOOP as u32) << 24, 1);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "reserved F3DEX2 command G_SPECIAL_2")]
    fn reserved_special_commands_trap_by_public_name() {
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(&mut rdram, 0x1000, (G_SPECIAL_2 as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    fn unrecognized_commands_trap_with_wire_context() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(&mut rdram, 0x1000, 0x0901_0203, 0x0405_0607);
        let panic = std::panic::catch_unwind(|| decode_display_list_f3dex2_ops(&rdram, 0x1000));
        assert!(panic.is_err());
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "render.gbi.geometry.command");
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].context.contains("F3DEX2"));
        assert!(events[0].context.contains("w0=0x09010203"));
    }

    #[test]
    #[should_panic(expected = "G_MOVEWORD unsupported index 0xff")]
    fn unknown_moveword_destination_traps_with_index() {
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | (0xff << 16),
            0,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_MOVEMEM unsupported index 0x0c")]
    fn unsupported_movemem_point_destination_traps_with_index() {
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEMEM as u32) << 24) | (1 << 19) | 0x0c,
            0,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "requires execute_display_list_f3dex2_ops with live RSP memory")]
    fn read_only_display_list_inspection_rejects_dma_io() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0, 8), 0x1080);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_LOAD_UCODE requires execute_display_list_f3dex2_ops")]
    fn read_only_display_list_inspection_rejects_load_ucode() {
        let mut rdram = vec![0u8; 0x3000];
        wr_cmd(&mut rdram, 0x1000, (G_RDPHALF_1 as u32) << 24, 0x1800);
        wr_cmd(&mut rdram, 0x1008, load_ucode_word(8), 0x2000);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "without the compound command's preceding G_RDPHALF_1")]
    fn load_ucode_rejects_a_missing_data_address_stage() {
        let mut rdram = vec![0u8; 0x3000];
        wr_cmd(&mut rdram, 0x1000, load_ucode_word(8), 0x2000);
        let _ = execute_display_list_f3dex2_ops(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            0x1000,
        );
    }

    #[test]
    #[should_panic(expected = "data-section size 7 is not a 64-bit multiple")]
    fn load_ucode_rejects_a_non_dma_granular_data_section() {
        let mut rdram = vec![0u8; 0x3000];
        wr_cmd(&mut rdram, 0x1000, (G_RDPHALF_1 as u32) << 24, 0x1800);
        wr_cmd(&mut rdram, 0x1008, load_ucode_word(7), 0x2000);
        let _ = execute_display_list_f3dex2_ops(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            0x1000,
        );
    }

    #[test]
    #[should_panic(expected = "is not a 64-bit multiple")]
    fn dma_io_rejects_non_multiple_transfer_size() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0, 7), 0x1080);
        let _ = execute_display_list_f3dex2_ops(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            0x1000,
        );
    }

    #[test]
    #[should_panic(expected = "is not 64-bit aligned")]
    fn dma_io_rejects_unaligned_dram_address() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0, 8), 0x1084);
        let _ = execute_display_list_f3dex2_ops(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            0x1000,
        );
    }

    #[test]
    #[should_panic(expected = "crosses its 4 KiB Dmem bank")]
    fn dma_io_rejects_a_transfer_that_crosses_rsp_banks() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0x0ff8, 16), 0x1080);
        let _ = execute_display_list_f3dex2_ops(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            0x1000,
        );
    }

    #[test]
    fn homogeneous_clip_codes_identify_each_shared_cull_plane() {
        assert_eq!(homogeneous_clip_code([0.0, 0.0, 0.0, 1.0]), 0);
        assert_eq!(homogeneous_clip_code([-2.0, 0.0, 0.0, 1.0]), CLIP_NEG_X);
        assert_eq!(homogeneous_clip_code([2.0, 0.0, 0.0, 1.0]), CLIP_POS_X);
        assert_eq!(homogeneous_clip_code([0.0, -2.0, 0.0, 1.0]), CLIP_NEG_Y);
        assert_eq!(homogeneous_clip_code([0.0, 2.0, 0.0, 1.0]), CLIP_POS_Y);
        assert_eq!(homogeneous_clip_code([0.0, 0.0, -2.0, 1.0]), CLIP_NEG_Z);
        assert_eq!(homogeneous_clip_code([0.0, 0.0, 2.0, 1.0]), CLIP_POS_Z);
    }

    #[test]
    fn line_clipping_interpolates_homogeneous_boundary_and_attributes() {
        let outside_clip = [-2.0, 0.0, 0.0, 1.0];
        let inside_clip = [0.0, 0.0, 0.0, 1.0];
        let outside = Vertex {
            r: 255,
            a: 255,
            w: 1.0,
            clip_code: homogeneous_clip_code(outside_clip),
            clip_position: Some(outside_clip),
            ..Default::default()
        };
        let inside = Vertex {
            b: 255,
            a: 255,
            w: 1.0,
            clip_code: homogeneous_clip_code(inside_clip),
            clip_position: Some(inside_clip),
            ..Default::default()
        };

        let [clipped, retained] = clip_line_to_homogeneous_volume(
            outside,
            inside,
            Some(centered_viewport()),
            ClipRatio::default(),
        )
        .unwrap();
        assert_eq!(clipped.clip_position, Some([-1.0, 0.0, 0.0, 1.0]));
        assert_eq!(clipped.x, 0.0);
        assert_eq!([clipped.r, clipped.g, clipped.b], [128, 0, 128]);
        assert_eq!(retained.x, 160.0);
        assert_eq!(clipped.clip_code, 0);
    }

    #[test]
    fn line_clipping_rejects_shared_outside_plane() {
        let a_clip = [-2.0, 0.0, 0.0, 1.0];
        let b_clip = [-3.0, 0.0, 0.0, 1.0];
        let vertex = |clip| Vertex {
            w: 1.0,
            clip_code: homogeneous_clip_code(clip),
            clip_position: Some(clip),
            ..Default::default()
        };
        assert!(clip_line_to_homogeneous_volume(
            vertex(a_clip),
            vertex(b_clip),
            Some(centered_viewport()),
            ClipRatio::default()
        )
        .is_none());
    }

    #[test]
    fn clip_ratio_expands_line_clip_planes_without_changing_cull_codes() {
        let outside_clip = [-4.0, 0.0, 0.0, 1.0];
        let inside_clip = [0.0, 0.0, 0.0, 1.0];
        let vertex = |clip| Vertex {
            w: 1.0,
            clip_code: homogeneous_clip_code(clip),
            clip_position: Some(clip),
            ..Default::default()
        };
        let [standard, _] = clip_line_to_homogeneous_volume(
            vertex(outside_clip),
            vertex(inside_clip),
            Some(centered_viewport()),
            ClipRatio::default(),
        )
        .unwrap();
        let [expanded, _] = clip_line_to_homogeneous_volume(
            vertex(outside_clip),
            vertex(inside_clip),
            Some(centered_viewport()),
            ClipRatio {
                neg_x: 3,
                neg_y: 3,
                pos_x: 3,
                pos_y: 3,
            },
        )
        .unwrap();
        assert_eq!(standard.clip_position, Some([-1.0, 0.0, 0.0, 1.0]));
        assert_eq!(expanded.clip_position, Some([-3.0, 0.0, 0.0, 1.0]));

        let mut state = lit_state();
        state.mvp = Some(identity());
        state.viewport = Some(centered_viewport());
        state.clip_ratio = ClipRatio {
            neg_x: 6,
            neg_y: 6,
            pos_x: 6,
            pos_y: 6,
        };
        assert_eq!(
            project_vertex(&state, 2.0, 0.0, 0.0).5,
            CLIP_POS_X,
            "G_CULLDL retained frustum codes are independent of gSPClipRatio"
        );
    }

    #[test]
    fn culled_nested_display_list_returns_to_its_caller() {
        const ROOT: usize = 0x1000;
        const CHILD: usize = 0x1100;
        const MATRIX: usize = 0x2000;
        const VIEWPORT: usize = 0x2100;
        const VERTICES: usize = 0x2200;
        let mut rdram = vec![0u8; 0x3000];
        wr_mtx(&mut rdram, MATRIX, identity());
        wr_centered_viewport(&mut rdram, VIEWPORT);
        // Slots 0..=1 share +X clipping. Slots 2..=4 form a visible triangle.
        for (slot, (x, y)) in [(2, 0), (2, 1), (-1, -1), (1, -1), (0, 1)]
            .into_iter()
            .enumerate()
        {
            wr_vtx(
                &mut rdram,
                VERTICES + slot * VTX_STRIDE,
                x,
                y,
                0,
                [255, 255, 255, 255],
            );
        }
        let mtx_len = ((64u32 - 1) / 8) << 19;
        let mut offset = ROOT;
        wr_cmd(&mut rdram, offset, movemem_viewport_word(), VIEWPORT as u32);
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_MTX as u32) << 24) | mtx_len | 0x07,
            MATRIX as u32,
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_VTX as u32) << 24) | (5 << 12) | (5 << 1),
            VERTICES as u32,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_DL as u32) << 24, CHILD as u32);
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_TRI1 as u32) << 24) | (2 << 17) | (3 << 9) | (4 << 1),
            0,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

        wr_cmd(&mut rdram, CHILD, (G_CULLDL as u32) << 24, 2);
        wr_cmd(
            &mut rdram,
            CHILD + 8,
            ((G_TRI1 as u32) << 24) | (2 << 17) | (3 << 9) | (4 << 1),
            0,
        );
        wr_cmd(&mut rdram, CHILD + 16, (G_ENDDL as u32) << 24, 0);

        let triangles = decode_display_list_f3dex2(&rdram, ROOT as u32).unwrap();
        assert_eq!(
            triangles.len(),
            1,
            "G_CULLDL must end only the child and resume the caller"
        );
    }

    fn decode_branch_z_fixture(threshold: u32) -> Vec<Triangle> {
        const ROOT: usize = 0x1000;
        const TARGET: usize = 0x1200;
        const VERTICES: usize = 0x2000;
        let mut rdram = vec![0u8; 0x3000];
        for (slot, (x, y, color)) in [
            (0, 0, [255, 0, 0, 255]),
            (4, 0, [255, 0, 0, 255]),
            (0, 4, [255, 0, 0, 255]),
            (10, 10, [0, 255, 0, 255]),
            (14, 10, [0, 255, 0, 255]),
            (10, 14, [0, 255, 0, 255]),
        ]
        .into_iter()
        .enumerate()
        {
            wr_vtx(&mut rdram, VERTICES + slot * VTX_STRIDE, x, y, 0, color);
        }
        let mut offset = ROOT;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_VTX as u32) << 24) | (6 << 12) | (6 << 1),
            VERTICES as u32,
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_MODIFYVTX as u32) << 24) | ((G_MWO_POINT_ZSCREEN as u32) << 16),
            0x0002_0000,
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            (G_RDPHALF_1 as u32) << 24,
            TARGET as u32,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_BRANCH_Z as u32) << 24, threshold);
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);
        wr_cmd(
            &mut rdram,
            TARGET,
            ((G_TRI1 as u32) << 24) | (3 << 17) | (4 << 9) | (5 << 1),
            0,
        );
        wr_cmd(&mut rdram, TARGET + 8, (G_ENDDL as u32) << 24, 0);
        decode_display_list_f3dex2(&rdram, ROOT as u32).unwrap()
    }

    #[test]
    fn branch_z_uses_exact_modified_screen_depth_and_tail_branches() {
        let taken = decode_branch_z_fixture(0x0002_0000);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].v[0].x, 10.0, "equality must take branch");

        let not_taken = decode_branch_z_fixture(0x0001_ffff);
        assert_eq!(not_taken.len(), 1);
        assert_eq!(not_taken[0].v[0].x, 0.0);
    }

    #[test]
    fn line3d_decodes_public_endpoint_and_half_pixel_width_fields() {
        let mut rdram = vec![0u8; 0x2100];
        wr_vtx(&mut rdram, 0x2000, 2, 4, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x2000 + VTX_STRIDE, 10, 4, 0, [0, 0, 255, 255]);
        // n=2, end=5 -> cache slots 3 and 4.
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (2 << 12) | (5 << 1),
            0x2000,
        );
        // F3DEX2 packs slot*2 in each endpoint byte and wd in the low byte.
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_LINE3D as u32) << 24) | (8 << 16) | (6 << 8) | 3,
            0,
        );
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

        let operations = decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
        let RenderOp::Line(line) = &operations[0] else {
            panic!("G_LINE3D must emit a typed line operation");
        };
        assert_eq!(line.v[0].x, 10.0, "wire endpoint order selects flat shade");
        assert_eq!(line.v[1].x, 2.0);
        assert_eq!(line.width, 3.0);
        assert!(!line.smooth_shading);
    }

    fn decode_geometry_fixture(rdram: &[u8], family: GeometryWireFamily) -> Vec<RenderOp> {
        let mut scratch = rdram.to_vec();
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let mut rdp_state = RdpDecodeState::default();
        execute_display_list_geometry_ops_admitted_with_rdp_state(
            &mut scratch,
            &mut rsp_memory,
            0x1000,
            &GeometryUcodeCatalog::default(),
            &mut rdp_state,
            family,
        )
        .unwrap()
    }

    fn equivalent_l3dex_line_fixture(family: GeometryWireFamily) -> Vec<RenderOp> {
        let mut rdram = vec![0u8; 0x2100];
        wr_vtx(&mut rdram, 0x2000, 2, 4, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x2000 + VTX_STRIDE, 10, 4, 0, [0, 0, 255, 255]);
        match family {
            GeometryWireFamily::L3dex => {
                // Public F3DEX envelope: v0*2 in bits 23:16 and
                // ((n << 10) | (sizeof(Vtx) * n - 1)) in the low halfword.
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((L3DEX_G_VTX as u32) << 24) | (6 << 16) | (2 << 10) | 31,
                    0x2000,
                );
                wr_cmd(
                    &mut rdram,
                    0x1008,
                    (L3DEX_G_LINE3D as u32) << 24,
                    (8 << 16) | (6 << 8) | 3,
                );
                wr_cmd(&mut rdram, 0x1010, (L3DEX_G_ENDDL as u32) << 24, 0);
            }
            GeometryWireFamily::L3dex2 => {
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((G_VTX as u32) << 24) | (2 << 12) | (5 << 1),
                    0x2000,
                );
                wr_cmd(
                    &mut rdram,
                    0x1008,
                    ((G_LINE3D as u32) << 24) | (8 << 16) | (6 << 8) | 3,
                    0,
                );
                wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
            }
            GeometryWireFamily::Fast3d
            | GeometryWireFamily::F3dex
            | GeometryWireFamily::F3dlx
            | GeometryWireFamily::F3dlxRej
            | GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej
            | GeometryWireFamily::F3dzex2 => {
                panic!("fixture requires a line microcode family")
            }
        }
        decode_geometry_fixture(&rdram, family)
    }

    #[test]
    fn l3dex_and_l3dex2_public_line_forms_normalize_to_identical_raster_input() {
        let legacy = equivalent_l3dex_line_fixture(GeometryWireFamily::L3dex);
        let modern = equivalent_l3dex_line_fixture(GeometryWireFamily::L3dex2);
        assert_eq!(legacy.len(), 1);
        assert_eq!(modern.len(), 1);

        let RenderOp::Line(legacy_line) = &legacy[0] else {
            panic!("L3DEX G_LINE3D must emit a typed line operation");
        };
        let RenderOp::Line(modern_line) = &modern[0] else {
            panic!("L3DEX2 G_LINE3D must emit a typed line operation");
        };
        assert_eq!(legacy_line.v[0].x, modern_line.v[0].x);
        assert_eq!(legacy_line.v[1].x, modern_line.v[1].x);
        assert_eq!(legacy_line.width, modern_line.width);
        assert_eq!(legacy_line.smooth_shading, modern_line.smooth_shading);
        let mut legacy_fb = crate::raster::Framebuffer::new(16, 10);
        legacy_fb.draw_line_no_depth(legacy_line);
        let mut modern_fb = crate::raster::Framebuffer::new(16, 10);
        modern_fb.draw_line_no_depth(modern_line);
        assert_eq!(legacy_fb.pixels, modern_fb.pixels);
        assert!(legacy_fb.pixels.iter().any(|component| *component != 0));
    }

    #[test]
    fn digest_selected_family_resolves_colliding_triangle_opcode() {
        let mut rdram = vec![0u8; 0x2100];
        for (slot, (x, y)) in [(1, 1), (5, 1), (1, 5)].into_iter().enumerate() {
            wr_vtx(&mut rdram, 0x2000 + slot * VTX_STRIDE, x, y, 0, [255; 4]);
        }
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x2000,
        );
        // This exact word is F3DEX2 G_TRI1 slots 0/1/2, but the public line
        // microcode contract interprets G_TRI1 as a validated NOOP.
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

        let polygon = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex2);
        let line = decode_geometry_fixture(&rdram, GeometryWireFamily::L3dex2);
        assert_eq!(polygon.len(), 1);
        assert!(matches!(polygon[0], RenderOp::Triangle(_)));
        assert!(line.is_empty());
    }

    fn equivalent_polygon_fixture(family: GeometryWireFamily) -> Vec<RenderOp> {
        let mut rdram = vec![0u8; 0x2100];
        for (slot, (x, y, color)) in [
            (1, 1, [255, 0, 0, 255]),
            (6, 1, [0, 255, 0, 255]),
            (1, 6, [0, 0, 255, 255]),
        ]
        .into_iter()
        .enumerate()
        {
            wr_vtx(&mut rdram, 0x2000 + slot * VTX_STRIDE, x, y, 0, color);
        }
        match family {
            GeometryWireFamily::Fast3d => {
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((L3DEX_G_VTX as u32) << 24) | (0x20 << 16) | 48,
                    0x2000,
                );
                wr_cmd(
                    &mut rdram,
                    0x1008,
                    (L3DEX_G_SETGEOMETRYMODE as u32) << 24,
                    LEGACY_G_SHADING_SMOOTH,
                );
                wr_cmd(
                    &mut rdram,
                    0x1010,
                    (L3DEX_G_TRI1 as u32) << 24,
                    (10 << 8) | 20,
                );
                wr_cmd(&mut rdram, 0x1018, (L3DEX_G_ENDDL as u32) << 24, 0);
            }
            GeometryWireFamily::F3dex
            | GeometryWireFamily::F3dlx
            | GeometryWireFamily::F3dlxRej => {
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((L3DEX_G_VTX as u32) << 24) | (3 << 10) | 47,
                    0x2000,
                );
                wr_cmd(
                    &mut rdram,
                    0x1008,
                    (L3DEX_G_SETGEOMETRYMODE as u32) << 24,
                    LEGACY_G_SHADING_SMOOTH,
                );
                wr_cmd(
                    &mut rdram,
                    0x1010,
                    (L3DEX_G_TRI1 as u32) << 24,
                    (2 << 8) | 4,
                );
                wr_cmd(&mut rdram, 0x1018, (L3DEX_G_ENDDL as u32) << 24, 0);
            }
            GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej => {
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
                    0x2000,
                );
                wr_cmd(
                    &mut rdram,
                    0x1008,
                    ((G_GEOMETRYMODE as u32) << 24) | 0x00ff_ffff,
                    G_SHADING_SMOOTH,
                );
                wr_cmd(
                    &mut rdram,
                    0x1010,
                    ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
                    0,
                );
                wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);
            }
            GeometryWireFamily::F3dzex2
            | GeometryWireFamily::L3dex
            | GeometryWireFamily::L3dex2 => {
                panic!("fixture requires a polygon microcode family")
            }
        }
        decode_geometry_fixture(&rdram, family)
    }

    #[test]
    fn admitted_polygon_families_public_triangle_forms_rasterize_identically() {
        let families = [
            GeometryWireFamily::Fast3d,
            GeometryWireFamily::F3dex,
            GeometryWireFamily::F3dlx,
            GeometryWireFamily::F3dlxRej,
            GeometryWireFamily::F3dex2,
            GeometryWireFamily::F3dex2NoN,
            GeometryWireFamily::F3dex2Rej,
            GeometryWireFamily::F3dlx2Rej,
        ];
        let pixels = families.map(|family| {
            let operations = equivalent_polygon_fixture(family);
            assert_eq!(operations.len(), 1);
            let RenderOp::Triangle(triangle) = &operations[0] else {
                panic!("polygon fixture must emit one typed triangle");
            };
            let mut framebuffer = crate::raster::Framebuffer::new(8, 8);
            framebuffer.draw_triangle(triangle);
            assert!(framebuffer.pixels.iter().any(|component| *component != 0));
            framebuffer.pixels
        });
        for pair in pixels.windows(2) {
            assert_eq!(pair[0], pair[1]);
        }
    }

    #[test]
    fn f3dlx_rej_uses_public_sixty_four_slot_cache_with_thirty_two_vertex_load_limit() {
        let mut rdram = vec![0u8; 0x2100];
        for (slot, (x, y)) in [(1, 1), (6, 1), (1, 6)].into_iter().enumerate() {
            wr_vtx(
                &mut rdram,
                0x2000 + slot * VTX_STRIDE,
                x,
                y,
                0,
                [255, 255, 255, 255],
            );
        }
        wr_cmd(
            &mut rdram,
            0x1000,
            ((L3DEX_G_VTX as u32) << 24) | (64 << 16) | (3 << 10) | 47,
            0x2000,
        );
        wr_cmd(
            &mut rdram,
            0x1008,
            (L3DEX_G_TRI1 as u32) << 24,
            (64 << 16) | (66 << 8) | 68,
        );
        wr_cmd(&mut rdram, 0x1010, (L3DEX_G_ENDDL as u32) << 24, 0);

        let rejected_by_f3dex =
            std::panic::catch_unwind(|| decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex));
        assert!(rejected_by_f3dex.is_err());
        let operations = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dlxRej);
        assert_eq!(operations.len(), 1);
        let RenderOp::Triangle(triangle) = &operations[0] else {
            panic!("F3DLX.Rej high-cache fixture must emit one triangle");
        };
        assert_eq!(triangle.v[0].x, 1.0);
        assert_eq!(triangle.v[1].x, 6.0);
        assert_eq!(triangle.v[2].y, 6.0);
    }

    #[test]
    fn f3dex2_rej_variants_load_all_sixty_four_vertices_in_one_command() {
        let mut rdram = vec![0u8; 0x2500];
        for (slot, (x, y)) in [(1, 1), (6, 1), (1, 6)].into_iter().enumerate() {
            wr_vtx(
                &mut rdram,
                0x2000 + (61 + slot) * VTX_STRIDE,
                x,
                y,
                0,
                [255, 255, 255, 255],
            );
        }
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (64 << 12) | (64 << 1),
            0x2000,
        );
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_TRI1 as u32) << 24) | (61 << 17) | (62 << 9) | (63 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

        let rejected_by_standard = std::panic::catch_unwind(|| {
            decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex2)
        });
        assert!(rejected_by_standard.is_err());
        for family in [GeometryWireFamily::F3dex2Rej, GeometryWireFamily::F3dlx2Rej] {
            let operations = decode_geometry_fixture(&rdram, family);
            assert_eq!(operations.len(), 1);
            let RenderOp::Triangle(triangle) = &operations[0] else {
                panic!("modern Rej high-cache fixture must emit one triangle");
            };
            assert_eq!(triangle.v[0].x, 1.0);
            assert_eq!(triangle.v[1].x, 6.0);
            assert_eq!(triangle.v[2].y, 6.0);
        }
    }

    #[test]
    fn f3dex2_non_disables_only_the_reference_near_admission_gate() {
        let mut cache = [Vertex::default(); 64];
        for vertex in &mut cache[..3] {
            vertex.w = 0.0;
            vertex.clip_position = Some([0.0, 0.0, -1.0, 0.0]);
        }
        assert!(resolve_tri_for_family(
            &cache,
            [0, 1, 2],
            GeometryWireFamily::F3dex2,
            0,
            ClipRatio::default(),
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_none());
        assert!(resolve_tri_for_family(
            &cache,
            [0, 1, 2],
            GeometryWireFamily::F3dex2NoN,
            0,
            ClipRatio::default(),
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_some());
    }

    #[test]
    #[should_panic(
        expected = "F3DLX2.Rej transformed G_VTX requires exact pixel-precision rounding that the public manuals do not specify"
    )]
    fn f3dlx2_rej_transformed_vertex_precision_remains_loud() {
        let mut state = lit_state();
        state.mvp = Some(identity());
        load_vertices(
            &[0; VTX_STRIDE],
            &mut state,
            0,
            1,
            0,
            GeometryWireFamily::F3dlx2Rej,
        );
    }

    #[test]
    fn public_f3dex2_variants_keep_special_opcodes_reserved() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(&mut rdram, 0x1000, (G_SPECIAL_1 as u32) << 24, 0);
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        for family in [
            GeometryWireFamily::F3dex2,
            GeometryWireFamily::F3dex2NoN,
            GeometryWireFamily::F3dex2Rej,
            GeometryWireFamily::F3dlx2Rej,
        ] {
            let result = std::panic::catch_unwind(|| decode_geometry_fixture(&rdram, family));
            assert!(
                result.is_err(),
                "{} accepted reserved G_SPECIAL_1",
                family.name()
            );
        }
    }

    #[test]
    #[should_panic(
        expected = "F3DZEX2 HLE admission requires a complete allowed-source specification for its family-specific continuation and branch commands"
    )]
    fn f3dzex2_digest_admission_is_loud_until_its_wire_is_allowed() {
        let mut catalog = GeometryUcodeCatalog::default();
        catalog.admit_text_for(
            GeometryWireFamily::F3dzex2,
            &[0x7a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
    }

    #[test]
    fn f3dlx_clipping_and_rej_cull_modes_are_family_specific() {
        let f3dlx = normalize_legacy_geometry_mode(
            GeometryWireFamily::F3dlx,
            LEGACY_G_CLIPPING | LEGACY_G_CULL_BACK,
        );
        assert_ne!(f3dlx & LEGACY_G_CLIPPING, 0);
        assert_ne!(f3dlx & G_CULL_BACK, 0);

        let f3dex_clipping = std::panic::catch_unwind(|| {
            normalize_legacy_geometry_mode(GeometryWireFamily::F3dex, LEGACY_G_CLIPPING)
        });
        assert!(f3dex_clipping.is_err());
        let rej_front_cull = std::panic::catch_unwind(|| {
            normalize_legacy_geometry_mode(GeometryWireFamily::F3dlxRej, LEGACY_G_CULL_FRONT)
        });
        assert!(rej_front_cull.is_err());
    }

    #[test]
    fn polygon_family_initial_state_matches_public_clip_and_reject_defaults() {
        let mut f3dlx = fresh_decode_state();
        initialize_geometry_family_state(&mut f3dlx, GeometryWireFamily::F3dlx);
        assert_ne!(f3dlx.geometry_mode & LEGACY_G_CLIPPING, 0);

        for family in [
            GeometryWireFamily::F3dlxRej,
            GeometryWireFamily::F3dex2,
            GeometryWireFamily::F3dex2NoN,
            GeometryWireFamily::F3dex2Rej,
            GeometryWireFamily::F3dlx2Rej,
        ] {
            let mut state = fresh_decode_state();
            initialize_geometry_family_state(&mut state, family);
            assert_eq!(
                state.clip_ratio,
                ClipRatio {
                    neg_x: 2,
                    neg_y: 2,
                    pos_x: 2,
                    pos_y: 2,
                },
                "{} must begin at public FRUSTRATIO_2",
                family.name()
            );
        }
    }

    #[test]
    fn f3dlx_rej_box_rejects_xy_and_far_but_not_near_vertices() {
        let ratio = ClipRatio {
            neg_x: 2,
            neg_y: 2,
            pos_x: 2,
            pos_y: 2,
        };
        let mut cache = [Vertex::default(); 64];
        for (slot, clip) in [
            [0.0, 0.0, -2.0, 1.0],
            [1.5, 0.0, 0.0, 1.0],
            [0.0, 1.5, 1.0, 1.0],
        ]
        .into_iter()
        .enumerate()
        {
            cache[slot].w = clip[3];
            cache[slot].clip_position = Some(clip);
        }
        assert!(resolve_tri_with_admission(
            &cache,
            [0, 1, 2],
            64,
            TriangleAdmission::RejectBox(ratio),
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_some());

        cache[1].clip_position = Some([2.1, 0.0, 0.0, 1.0]);
        assert!(resolve_tri_with_admission(
            &cache,
            [0, 1, 2],
            64,
            TriangleAdmission::RejectBox(ratio),
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_none());
        cache[1].clip_position = Some([0.0, 0.0, 1.1, 1.0]);
        assert!(resolve_tri_with_admission(
            &cache,
            [0, 1, 2],
            64,
            TriangleAdmission::RejectBox(ratio),
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_none());
    }

    #[test]
    #[should_panic(
        expected = "F3DLX transformed G_VTX requires exact pixel-precision rounding that the public manuals do not specify"
    )]
    fn f3dlx_transformed_vertex_precision_remains_a_loud_frontier() {
        let mut state = lit_state();
        state.mvp = Some(identity());
        load_vertices(
            &[0; VTX_STRIDE],
            &mut state,
            0,
            1,
            0,
            GeometryWireFamily::F3dlx,
        );
    }

    #[test]
    #[should_panic(expected = "unsupported Fast3D command byte 0xb1")]
    fn unpublished_historical_fast3d_quadrangle_form_remains_loud() {
        normalize_geometry_command(
            GeometryWireFamily::Fast3d,
            (F3DEX_G_TRI2 as u32) << 24,
            0,
            0x1000,
        );
    }

    fn published_quadrangle_fixture(family: GeometryWireFamily) -> Vec<RenderOp> {
        let mut rdram = vec![0u8; 0x2100];
        for (slot, (x, y)) in [(1, 1), (6, 1), (6, 6), (1, 6)].into_iter().enumerate() {
            wr_vtx(
                &mut rdram,
                0x2000 + slot * VTX_STRIDE,
                x,
                y,
                0,
                [255, 255, 255, 255],
            );
        }
        let first = (1 << 9) | (2 << 1);
        let second = (2 << 9) | (3 << 1);
        match family {
            GeometryWireFamily::F3dex => {
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((L3DEX_G_VTX as u32) << 24) | (4 << 10) | 63,
                    0x2000,
                );
                // Current public gSP1Quadrangle lowers to G_TRI2 with the
                // v0,v1,v2 and v0,v2,v3 split for flat-shade flag zero.
                wr_cmd(
                    &mut rdram,
                    0x1008,
                    ((F3DEX_G_TRI2 as u32) << 24) | first,
                    second,
                );
                wr_cmd(&mut rdram, 0x1010, (L3DEX_G_ENDDL as u32) << 24, 0);
            }
            GeometryWireFamily::F3dex2 => {
                wr_cmd(
                    &mut rdram,
                    0x1000,
                    ((G_VTX as u32) << 24) | (4 << 12) | (4 << 1),
                    0x2000,
                );
                wr_cmd(&mut rdram, 0x1008, ((G_QUAD as u32) << 24) | first, second);
                wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
            }
            _ => panic!("fixture requires a published F3DEX/F3DEX2 quadrangle form"),
        }
        decode_geometry_fixture(&rdram, family)
    }

    #[test]
    fn published_f3dex_quadrangle_emulation_matches_f3dex2_quad_raster() {
        let legacy = published_quadrangle_fixture(GeometryWireFamily::F3dex);
        let modern = published_quadrangle_fixture(GeometryWireFamily::F3dex2);
        assert_eq!(legacy.len(), 2);
        assert_eq!(modern.len(), 2);

        let raster = |operations: &[RenderOp]| {
            let mut framebuffer = crate::raster::Framebuffer::new(8, 8);
            for operation in operations {
                let RenderOp::Triangle(triangle) = operation else {
                    panic!("quadrangle fixture must lower to two triangles");
                };
                framebuffer.draw_triangle(triangle);
            }
            framebuffer.pixels
        };
        assert_eq!(raster(&legacy), raster(&modern));
    }

    #[test]
    fn digest_family_disambiguates_opcode_01_matrix_from_vertex() {
        // The same complete first word is a public 64-byte projection matrix
        // DMA under base/F3DEX envelopes and a 16-vertex load ending at slot
        // 32 under F3DEX2. Only admitted digest identity can choose safely.
        let colliding_w0 = 0x0101_0040;
        let (fast_w0, _) =
            normalize_geometry_command(GeometryWireFamily::Fast3d, colliding_w0, 0x2000, 0x1000);
        let (f3dex_w0, _) =
            normalize_geometry_command(GeometryWireFamily::F3dex, colliding_w0, 0x2000, 0x1000);
        let (modern_w0, _) =
            normalize_geometry_command(GeometryWireFamily::F3dex2, colliding_w0, 0x2000, 0x1000);
        assert_eq!(fast_w0 >> 24, u32::from(G_MTX));
        assert_eq!(f3dex_w0 >> 24, u32::from(G_MTX));
        assert_eq!(modern_w0 >> 24, u32::from(G_VTX));
    }

    #[test]
    fn geometry_catalog_reports_only_admitted_families_and_rejects_collisions() {
        let mut catalog = GeometryUcodeCatalog::default();
        catalog.admit_text_for(
            GeometryWireFamily::Fast3d,
            &[0x01; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dex,
            &[0x02; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dlx,
            &[0x03; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dlxRej,
            &[0x04; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dex2,
            &[0x05; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dex2NoN,
            &[0x06; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dex2Rej,
            &[0x07; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::F3dlx2Rej,
            &[0x08; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        catalog.admit_text_for(
            GeometryWireFamily::L3dex,
            &[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        assert_eq!(
            catalog.supported_ucodes(),
            &[
                UcodeId::Fast3d,
                UcodeId::F3dex,
                UcodeId::F3dlx,
                UcodeId::F3dlxRej,
                UcodeId::F3dex2,
                UcodeId::F3dex2NoN,
                UcodeId::F3dex2Rej,
                UcodeId::F3dlx2Rej,
                UcodeId::L3dex
            ]
        );
        catalog.admit_text_for(
            GeometryWireFamily::L3dex2,
            &[0x22; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        );
        assert_eq!(
            catalog.supported_ucodes(),
            &[
                UcodeId::Fast3d,
                UcodeId::F3dex,
                UcodeId::F3dlx,
                UcodeId::F3dlxRej,
                UcodeId::F3dex2,
                UcodeId::F3dex2NoN,
                UcodeId::F3dex2Rej,
                UcodeId::F3dlx2Rej,
                UcodeId::L3dex,
                UcodeId::L3dex2
            ]
        );

        let digest = [0x5a; 32];
        catalog.admit_sha256_for(GeometryWireFamily::L3dex, digest);
        let collision = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            catalog.admit_sha256_for(GeometryWireFamily::L3dex2, digest);
        }));
        assert!(collision.is_err());
    }

    #[test]
    #[should_panic(expected = "G_LINE3D cache indices must use F3DEX2 v*2 encoding")]
    fn malformed_line3d_endpoint_encoding_traps_by_name() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(&mut rdram, 0x1000, ((G_LINE3D as u32) << 24) | (1 << 16), 0);
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "without a preceding G_RDPHALF_1 target")]
    fn taken_branch_z_without_staged_target_traps_by_name() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, (G_BRANCH_Z as u32) << 24, u32::MAX);
        decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
    }

    #[test]
    fn pop_matrix_n_restores_the_requested_modelview_depth() {
        const DL: usize = 0x1000;
        const A: usize = 0x2000;
        const B: usize = 0x2100;
        const C: usize = 0x2200;
        let mut rdram = vec![0u8; 0x3000];
        let mut a = identity();
        a[3][0] = 1.0;
        let mut b = identity();
        b[3][0] = 2.0;
        let mut c = identity();
        c[3][0] = 3.0;
        wr_mtx(&mut rdram, A, a);
        wr_mtx(&mut rdram, B, b);
        wr_mtx(&mut rdram, C, c);
        let mtx_len = ((64u32 - 1) / 8) << 19;
        let command = |wire: u32| ((G_MTX as u32) << 24) | mtx_len | wire;
        wr_cmd(&mut rdram, DL, command(0x03), A as u32); // LOAD
        wr_cmd(&mut rdram, DL + 8, command(0x02), B as u32); // LOAD|PUSH
        wr_cmd(&mut rdram, DL + 16, command(0x02), C as u32); // LOAD|PUSH
        wr_cmd(
            &mut rdram,
            DL + 24,
            ((G_POPMTX as u32) << 24) | mtx_len | 2,
            2 * 64,
        );
        wr_cmd(&mut rdram, DL + 32, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
        assert_eq!(state.modelview, a);
        assert!(state.mv_stack.is_empty());
    }

    #[test]
    fn set_convert_decodes_all_signed_nine_bit_fields_in_raw_and_f3dex_streams() {
        let expected = [-256, -43, -1, 222, 114, 255];
        let (w0, w1) = set_convert_command(expected);
        assert_eq!(ConvertState::decode(w0, w1).coefficients, expected);

        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, w0, w1);
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_TEXRECT as u32) << 24) | (4 << 12) | 4,
            0,
        );
        wr_cmd(&mut rdram, 0x1010, 0, 0x0400_0400);
        wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);

        for ops in [
            decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap(),
            decode_raw_rdp_ops(&rdram, 0x1000).unwrap(),
        ] {
            let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
                panic!("expected texture rectangle after G_SETCONVERT");
            };
            assert_eq!(rectangle.combiner.convert.coefficients, expected);
        }

        let mut command_only = vec![0u8; 8];
        wr_cmd(&mut command_only, 0, w0, w1);
        validate_raw_rdp_command_range(&command_only, 0, 8).unwrap();
    }

    #[test]
    fn set_key_commands_converge_across_raw_and_f3dex_streams() {
        let expected = KeyState {
            center: [10, 20, 30],
            scale: [40, 50, 60],
            width: [0x101, 0x100, 0x080],
        };
        let set_gb = (
            ((G_SETKEYGB as u32) << 24)
                | (u32::from(expected.width[1]) << 12)
                | u32::from(expected.width[2]),
            (u32::from(expected.center[1]) << 24)
                | (u32::from(expected.scale[1]) << 16)
                | (u32::from(expected.center[2]) << 8)
                | u32::from(expected.scale[2]),
        );
        let set_r = (
            (G_SETKEYR as u32) << 24,
            (u32::from(expected.width[0]) << 16)
                | (u32::from(expected.center[0]) << 8)
                | u32::from(expected.scale[0]),
        );

        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, set_gb.0, set_gb.1);
        wr_cmd(&mut rdram, 0x1008, set_r.0, set_r.1);
        wr_cmd(
            &mut rdram,
            0x1010,
            ((G_TEXRECT as u32) << 24) | (4 << 12) | 4,
            0,
        );
        wr_cmd(&mut rdram, 0x1018, 0, 0x0400_0400);
        wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

        for ops in [
            decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap(),
            decode_raw_rdp_ops(&rdram, 0x1000).unwrap(),
        ] {
            let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
                panic!("expected texture rectangle after G_SETKEYGB/G_SETKEYR");
            };
            assert_eq!(rectangle.combiner.key, expected);
        }

        validate_raw_rdp_command_range(&rdram, 0x1000, 0x1010).unwrap();
        assert_eq!(expected.alpha_from_key_prime([10.0, 0.0, 0.0]), 0.5);
    }

    #[test]
    fn modify_vertex_updates_all_public_post_transform_cache_fields() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x3000, 1, 2, 3, [4, 5, 6, 7]);
        wr_vtx(&mut rdram, 0x3010, 20, 2, 3, [8, 9, 10, 11]);
        wr_vtx(&mut rdram, 0x3020, 1, 20, 3, [12, 13, 14, 15]);
        let mut offset = 0x1000;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        offset += 8;
        for (where_field, value) in [
            (G_MWO_POINT_RGBA, 0xA1_B2_C3_D4),
            (G_MWO_POINT_ST, 0x0140_FF60), // s=10.0, t=-5.0 in S10.5
            (G_MWO_POINT_XYSCREEN, 0x0032_FFF3), // x=12.5, y=-3.25 in S13.2
            (G_MWO_POINT_ZSCREEN, 0x01FF_8000), // z=511.5 in 16.16
        ] {
            wr_cmd(
                &mut rdram,
                offset,
                ((G_MODIFYVTX as u32) << 24) | ((where_field as u32) << 16),
                value,
            );
            offset += 8;
        }
        wr_cmd(
            &mut rdram,
            offset,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

        let triangles = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(triangles.len(), 1);
        let vertex = triangles[0].v[0];
        assert_eq!(
            [vertex.r, vertex.g, vertex.b, vertex.a],
            [0xA1, 0xB2, 0xC3, 0xD4]
        );
        assert_eq!((vertex.s, vertex.t), (10.0, -5.0));
        assert_eq!((vertex.x, vertex.y), (12.5, -3.25));
        assert_eq!(vertex.z, 511.5);
    }

    #[test]
    #[should_panic(expected = "G_MODIFYVTX cache slot 0 uses unsupported where field 0x20")]
    fn modify_vertex_unknown_destination_traps_by_name() {
        let mut rdram = vec![0u8; 0x2000];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MODIFYVTX as u32) << 24) | (0x20 << 16),
            0,
        );
        let _ = decode_display_list_f3dex2(&rdram, 0x1000);
    }

    #[test]
    fn ordered_ops_preserve_color_target_fill_and_full_sync() {
        let mut rdram = vec![0u8; 0x3000];
        let mut offset = 0x1000;
        // Full other-mode write with G_CYC_FILL in high bits 20..21.
        wr_cmd(
            &mut rdram,
            offset,
            ((G_RDPSETOTHERMODE as u32) << 24) | (3 << 20),
            0,
        );
        offset += 8;
        // RGBA16, width=4, target=0x2000.
        wr_cmd(
            &mut rdram,
            offset,
            ((G_SETCIMG as u32) << 24) | (2 << 19) | 3,
            0x2000,
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            (G_SETFILLCOLOR as u32) << 24,
            0xf801_003f,
        );
        offset += 8;
        // Inclusive rectangle (0,0)..(2,1), packed as quarter pixels.
        wr_cmd(
            &mut rdram,
            offset,
            ((G_FILLRECT as u32) << 24) | ((2 * 4) << 12) | (1 * 4),
            0,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_RDPFULLSYNC as u32) << 24, 0);
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

        let ops = decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(
            ops[0],
            RenderOp::SetColorImage(ColorImage {
                format: 0,
                size: 2,
                width: 4,
                address: 0x2000,
            })
        ));
        assert!(matches!(
            ops[1],
            RenderOp::FillRectangle(FillRectangle {
                ulx: 0.0,
                uly: 0.0,
                lrx: 2.0,
                lry: 1.0,
                fill_color: 0xf801_003f,
                cycle_type: CycleType::Fill,
                ..
            })
        ));
        assert!(matches!(ops[2], RenderOp::FullSync));
        assert!(decode_display_list_f3dex2(&rdram, 0x1000)
            .unwrap()
            .is_empty());
    }

    /// Pack raw `gsDPSetCombineLERP` selectors exactly like public gbi.h's
    /// `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1` macros (lines 3543-3565).
    fn combine_cmd(
        rgb0: [u32; 4],
        alpha0: [u32; 4],
        rgb1: [u32; 4],
        alpha1: [u32; 4],
    ) -> (u32, u32) {
        let w0 = ((G_SETCOMBINE as u32) << 24)
            | ((rgb0[0] & 0x0f) << 20)
            | ((rgb0[2] & 0x1f) << 15)
            | ((alpha0[0] & 0x07) << 12)
            | ((alpha0[2] & 0x07) << 9)
            | ((rgb1[0] & 0x0f) << 5)
            | (rgb1[2] & 0x1f);
        let w1 = ((rgb0[1] & 0x0f) << 28)
            | ((rgb1[1] & 0x0f) << 24)
            | ((alpha1[0] & 0x07) << 21)
            | ((alpha1[2] & 0x07) << 18)
            | ((rgb0[3] & 0x07) << 15)
            | ((alpha0[1] & 0x07) << 12)
            | ((alpha0[3] & 0x07) << 9)
            | ((rgb1[3] & 0x07) << 6)
            | ((alpha1[1] & 0x07) << 3)
            | (alpha1[3] & 0x07);
        (w0, w1)
    }

    #[test]
    fn setcombine_and_color_registers_are_snapshotted_on_triangles() {
        // Fail-against-bug: before this change all three commands fell into
        // the skip arm, so Triangle had no mode/primitive/environment state
        // and the rasterizer could only hardwire TEXEL0*SHADE.
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x3000, 2, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x3010, 12, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x3020, 7, 12, 0, [255, 255, 255, 255]);

        // Cycle 0 G_CC_BLENDI RGB: (ENV-SHADE)*TEXEL0+SHADE.
        // Alpha: TEXEL0*PRIMITIVE. Cycle 1 deliberately uses distinct
        // non-zero selectors in every field so a shifted/masked decode fails.
        let (cc0, cc1) = combine_cmd([5, 4, 1, 4], [1, 7, 3, 7], [3, 5, 11, 1], [5, 4, 6, 3]);
        let mut off = 0x1000;
        wr_cmd(&mut rdram, off, cc0, cc1);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_SETPRIMCOLOR as u32) << 24) | 0x7f,
            0x11_22_33_44,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_SETENVCOLOR as u32) << 24, 0xa0_b0_c0_d0);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1);
        let cc = tris[0].combiner;
        assert_eq!(cc.primitive, [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(cc.environment, [0xa0, 0xb0, 0xc0, 0xd0]);
        assert_eq!(cc.prim_lod_fraction, 0x7f);
        assert_eq!(
            cc.mode.cycles[0].rgb,
            [
                ColorSource::Environment,
                ColorSource::Shade,
                ColorSource::Texel0,
                ColorSource::Shade,
            ]
        );
        assert_eq!(
            cc.mode.cycles[0].alpha,
            [
                AlphaSource::Texel0,
                AlphaSource::Zero,
                AlphaSource::Primitive,
                AlphaSource::Zero,
            ]
        );
        assert_eq!(
            cc.mode.cycles[1].rgb,
            [
                ColorSource::Primitive,
                ColorSource::Environment,
                ColorSource::ShadeAlpha,
                ColorSource::Texel0,
            ]
        );
        assert_eq!(
            cc.mode.cycles[1].alpha,
            [
                AlphaSource::Environment,
                AlphaSource::Shade,
                AlphaSource::PrimLodFraction,
                AlphaSource::Primitive,
            ]
        );
    }

    /// Plant a full 16-byte `Vtx` (`ob` x/y/z at 0/2/4, color at 12) at `off`
    /// so a `G_VTX` + `G_TRI1` can resolve a real triangle.
    fn wr_vtx(rdram: &mut [u8], off: usize, x: i16, y: i16, z: i16, rgba: [u8; 4]) {
        wr_i16(rdram, off, x);
        wr_i16(rdram, off + 2, y);
        wr_i16(rdram, off + 4, z);
        for (i, &c) in rgba.iter().enumerate() {
            rdram[(off + 12 + i) ^ 3] = c;
        }
    }

    /// Write a 64-byte fixed-point `Mtx` at `off` from an f32 `[row][col]`
    /// matrix, matching `read_mtx`'s layout: element (r,c) integer half at
    /// `off + (r*4+c)*2`, fractional half at `off + 32 + (r*4+c)*2`, both
    /// through the recomp `^3` swizzle (via `wr_i16`).
    fn wr_mtx(rdram: &mut [u8], off: usize, m: [[f32; 4]; 4]) {
        for (r, row) in m.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                let elem = r * 4 + c;
                let fixed = (*value * 65536.0).round() as i32;
                let int_half = (fixed >> 16) as i16;
                let frac_half = (fixed & 0xFFFF) as u16;
                wr_i16(rdram, off + elem * 2, int_half);
                wr_i16(rdram, off + 32 + elem * 2, frac_half as i16);
            }
        }
    }

    /// Encode the F3DEX2 partial other-mode range used by
    /// `gSPSetOtherMode` (`gbi.h:3353-3369`).
    fn other_mode_cmd(opcode: u8, shift: u32, length: u32) -> u32 {
        ((opcode as u32) << 24) | ((32 - shift - length) << 8) | (length - 1)
    }

    /// Fails against the pre-fix name-table-only decoder: several partial H/L
    /// writes must merge without clobbering each other, and the resulting
    /// cycle/filter/dither/alpha/coverage/Z/blender state plus blend-alpha
    /// threshold must be snapshotted onto the emitted triangle.
    #[test]
    fn other_mode_partial_updates_are_decoded_and_carried_per_triangle() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x2000, 2, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x2010, 12, 2, 0, [255, 255, 255, 255]);
        wr_vtx(&mut rdram, 0x2020, 7, 12, 0, [255, 255, 255, 255]);

        let mut off = 0x1000;
        let mut emit = |w0: u32, w1: u32| {
            wr_cmd(&mut rdram, off, w0, w1);
            off += 8;
        };
        emit(((G_VTX as u32) << 24) | (3 << 12) | (3 << 1), 0x2000);
        emit(other_mode_cmd(G_SETOTHERMODE_H, 20, 2), 2 << 20); // G_CYC_COPY
        emit(other_mode_cmd(G_SETOTHERMODE_H, 12, 2), 2 << 12); // G_TF_BILERP
        emit(other_mode_cmd(G_SETOTHERMODE_H, 6, 2), 3 << 6); // G_CD_DISABLE
        emit(other_mode_cmd(G_SETOTHERMODE_H, 4, 2), 2 << 4); // G_AD_NOISE
        emit(other_mode_cmd(G_SETOTHERMODE_L, 0, 2), 1); // G_AC_THRESHOLD

        let blender = (1 << 30) | (2 << 26) | (3 << 22) | (2 << 28) | (1 << 24) | (3 << 16);
        let render = blender | 0x0010 | 0x0020 | 0x0100 | 0x0800 | 0x1000 | 0x2000 | 0x4000;
        emit(other_mode_cmd(G_SETOTHERMODE_L, 3, 29), render);
        emit((G_SETBLENDCOLOR as u32) << 24, 0x0102_0380);
        emit((G_TRI1 as u32) << 24 | (1 << 9) | (2 << 1), 0);
        emit(other_mode_cmd(G_SETOTHERMODE_L, 0, 2), 0); // G_AC_NONE
        emit((G_TRI1 as u32) << 24 | (1 << 9) | (2 << 1), 0);
        emit((G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 2);
        let mode = tris[0].other_mode;
        assert_eq!(mode.cycle_type(), CycleType::Copy);
        assert_eq!(mode.texture_filter(), TextureFilter::Bilinear);
        assert_eq!(mode.rgb_dither(), RgbDither::Disabled);
        assert_eq!(mode.alpha_dither(), AlphaDither::Noise);
        assert_eq!(mode.alpha_compare(), AlphaCompare::Threshold);
        assert_eq!(mode.blend_color_alpha, 0x80);
        assert!(mode.depth_compare_enabled());
        assert!(mode.depth_update_enabled());
        assert_eq!(mode.coverage_destination(), CoverageDestination::Wrap);
        assert_eq!(mode.depth_mode(), DepthMode::Translucent);
        assert!(mode.coverage_times_alpha());
        assert!(mode.alpha_coverage_select());
        assert!(mode.force_blend());
        assert_eq!(
            mode.blender_cycle_1(),
            BlenderCycle {
                color_a: 1,
                alpha_a: 2,
                color_b: 3,
                alpha_b: 0,
            }
        );
        assert_eq!(
            mode.blender_cycle_2(),
            BlenderCycle {
                color_a: 2,
                alpha_a: 1,
                color_b: 0,
                alpha_b: 3,
            }
        );
        assert_eq!(tris[1].other_mode.alpha_compare(), AlphaCompare::None);
        assert_eq!(tris[1].other_mode.raw_high(), mode.raw_high());
        assert_eq!(tris[1].other_mode.raw_low(), mode.raw_low() & !3);
    }

    /// OoT's public G_RM_* constants embed G_AC_DITHER outside the nominal
    /// gDPSetRenderMode bits-3..31 range. This fails if the decoder masks w1
    /// instead of following the RSP/RT64 full-data OR behavior.
    #[test]
    fn render_mode_update_keeps_embedded_alpha_dither_bits() {
        let w0 = other_mode_cmd(G_SETOTHERMODE_L, 3, 29);
        let updated = update_other_mode_word(0, w0, 3 | 0x0010).unwrap();
        let mode = OtherMode::from_raw(0, updated, 0);
        assert_eq!(mode.alpha_compare(), AlphaCompare::Dither);
        assert!(mode.depth_compare_enabled());
    }

    /// Fails against the original decoder, which loudly skipped opcode 0xEF
    /// and emitted every triangle with overwrite-only default state.
    #[test]
    fn full_othermode_command_snapshots_both_blender_cycles_on_triangle() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x2000, 2, 2, 0, [255, 255, 255, 128]);
        wr_vtx(&mut rdram, 0x2010, 12, 2, 0, [255, 255, 255, 128]);
        wr_vtx(&mut rdram, 0x2020, 7, 12, 0, [255, 255, 255, 128]);

        // G_CYC_2CYCLE plus the standard XLU tuple in both cycles:
        // IN*A_IN + MEM*(1-A). Selector positions are exactly GBL_c1/c2
        // (gbi.h:624-627); FORCE_BL is gbi.h:609.
        let high = 1 << 20;
        let low = (1 << 22) | (1 << 20) | 0x4000;
        let mut off = 0x1000;
        wr_cmd(
            &mut rdram,
            off,
            ((G_RDPSETOTHERMODE as u32) << 24) | high,
            low,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_SETBLENDCOLOR as u32) << 24, 0x1020_3040);
        off += 8;
        wr_cmd(&mut rdram, off, (G_SETFOGCOLOR as u32) << 24, 0x5060_7080);
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x2000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (0 << 17) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1);
        let blender = tris[0].blender;
        assert_eq!(blender.cycle_count, 2);
        assert!(blender.force_blend);
        assert_eq!(blender.blend_color, [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(blender.fog_color, [0x50, 0x60, 0x70, 0x80]);
        for cycle in blender.cycles {
            assert_eq!(cycle.p, BlendColorInput::Combined);
            assert_eq!(cycle.a, BlendAlphaInput::Combined);
            assert_eq!(cycle.m, BlendColorInput::Framebuffer);
            assert_eq!(cycle.b, BlendBInput::OneMinusA);
        }
    }

    /// Partial setters use F3DEX2's inverted shift field, not the older F3D
    /// direct shift. These exact words are what gDPSetCycleType and
    /// gDPSetRenderMode emit through gSPSetOtherMode (gbi.h:3353-3369).
    #[test]
    fn partial_othermode_commands_patch_the_logical_bit_ranges() {
        let cycle_type_w0 = ((0xE3u32) << 24) | (10 << 8) | 1;
        let high = update_other_mode_word(0, cycle_type_w0, 1 << 20).unwrap();
        assert_eq!((high >> 20) & 3, 1);

        let render_mode_w0 = ((0xE2u32) << 24) | 28;
        let render_mode = (1 << 22) | (1 << 20) | 0x4000;
        let low = update_other_mode_word(0b101, render_mode_w0, render_mode).unwrap();
        assert_eq!(low & 0b111, 0b101, "bits below render mode stay intact");
        assert_eq!(low & !0b111, render_mode);
    }

    #[test]
    fn setscissor_decodes_quarter_pixel_edges_on_emitted_triangle() {
        let mut rdram = vec![0u8; 0x4000];
        wr_vtx(&mut rdram, 0x2000, 2, 3, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x2010, 12, 3, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x2020, 2, 13, 0, [0, 0, 255, 255]);

        let raw_ulx = 5u32; // 1.25 px
        let raw_uly = 10u32; // 2.5 px
        let raw_lrx = 43u32; // 10.75 px
        let raw_lry = 48u32; // 12 px
        let mut off = 0x1000;
        wr_cmd(
            &mut rdram,
            off,
            ((G_SETSCISSOR as u32) << 24) | (raw_ulx << 12) | raw_uly,
            (1 << 25) | (1 << 24) | (raw_lrx << 12) | raw_lry,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x2000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1);
        assert_eq!(
            tris[0].scissor,
            Some(ScissorRect {
                ulx: 1.25,
                uly: 2.5,
                lrx: 10.75,
                lry: 12.0,
                field: true,
                keep_odd: true,
            })
        );
    }

    // --- Perspective * view * model projection regression ----------------
    //
    // Fails against the pre-fix decoder, which transposed each `Mtx` on read
    // AND accumulated the projection product in the wrong (proj-first) order.
    // The two errors cancel for a single diagonal/symmetric matrix -- all the
    // older f3dex2_replay fixture exercised -- so the bug slipped through, but
    // for a real guPerspective * guLookAt * model chain the net composed MVP
    // came out as the TRANSPOSE of the true one: clip `w` collapsed to tiny,
    // sign-flipping values (~30, ~-13, ~-59) and the perspective divide flung
    // a vertex that belongs near screen-center to ~800+ px off the 320x240
    // screen. This test drives the exact live-OoT-gameplay P/V/M matrices
    // through the full decode and asserts the vertex now lands on-screen with
    // a coherent positive `w`.
    #[test]
    fn perspective_view_model_projects_vertex_on_screen() {
        let mut rdram = vec![0u8; 0x8000];

        // A CLEAN, self-consistent row-vector (N64 [row][col]) setup whose
        // on-screen anchors are derived INDEPENDENTLY, NOT reverse-engineered
        // to fit a transposed apply. guPerspective(fovy=60, aspect=4/3,
        // near=10, far=1000): projective term [2][3]=-1, depth translate at
        // [3][2]. The modelview is deliberately ASYMMETRIC (a 20° rotation
        // about Y + a translation to (30,-15,-120)) so `mvp != mvp^T` -- a
        // pure-translation/diagonal MVP would be transpose-invariant and could
        // NOT distinguish the bug from the fix.
        //
        // Under the CORRECT row-vector transform `clip = v · (M · P)`:
        //   - object origin (0,0,0) -> w=120, screen (211.96, 145.98);
        //   - vertex (10,20,0)      -> w=123.42, screen (226.35, 111.58).
        // The pre-fix COLUMN-vector apply (`(M·P)·v`) is the transpose: it
        // sends (10,20,0) to w=-9.9 (behind the camera) / px=(-42.7, 539.7),
        // i.e. off-screen -- the fanning-triangle bug.
        let persp = [
            [1.299038, 0.0, 0.0, 0.0],
            [0.0, 1.732051, 0.0, 0.0],
            [0.0, 0.0, -1.020202, -1.0],
            [0.0, 0.0, -20.202_02, 0.0],
        ];
        // Asymmetric modelview: rot(20° about Y) then translate(30,-15,-120),
        // in hardware [row][col] row-vector layout.
        let model = [
            [0.939693, 0.0, -0.342020, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.342020, 0.0, 0.939693, 0.0],
            [30.0, -15.0, -120.0, 1.0],
        ];

        // Triangle vertices (model space): center, +x/+y offset, +x.
        wr_vtx(&mut rdram, 0x3000, 0, 0, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 10, 20, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 20, 0, 0, [0, 0, 255, 255]);

        wr_mtx(&mut rdram, 0x2000, persp);
        wr_mtx(&mut rdram, 0x2200, model);
        wr_centered_viewport(&mut rdram, 0x2400);

        // G_MTX param bytes (wire = params ^ G_MTX_PUSH):
        //  perspective LOAD:  PROJECTION|LOAD   = 0x06 -> wire 0x07
        //  model LOAD:        LOAD (modelview)  = 0x02 -> wire 0x03
        let mtx_len = ((64u32 - 1) / 8) << 19;
        let mtx_cmd = |idx: u32| ((G_MTX as u32) << 24) | mtx_len | idx;
        let mut off = 0x1000;
        wr_cmd(&mut rdram, off, movemem_viewport_word(), 0x2400);
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x07), 0x2000); // persp LOAD
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x03), 0x2200); // model LOAD
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1, "expected one transformed triangle");
        // Explicit 320x240 viewport: NDC*160+160 / *120+120.
        // Independently-derived anchors (see numpy in the doc above):
        //   v0 (object origin) -> (211.96, 145.98), w=120.
        //   v1 (10,20,0)       -> (226.35, 111.58), w=123.42.
        let v0 = &tris[0].v[0];
        assert!(
            (v0.x - 211.96).abs() < 0.5 && (v0.y - 145.98).abs() < 0.5,
            "object-origin vertex must land at its independently-derived anchor \
             (~211.96, ~145.98) under the correct row-vector MVP; got ({}, {}) \
             (the transposed/column-vector apply misses it, off-screen)",
            v0.x,
            v0.y
        );
        let v1 = &tris[0].v[1];
        assert!(
            (v1.x - 226.35).abs() < 0.5 && (v1.y - 111.58).abs() < 0.5,
            "offset vertex drifted from the independently-derived on-screen \
             anchor (~226.35, ~111.58); got ({}, {}) -- a re-transpose sends it \
             to px≈(-42.7, 539.7), off-screen",
            v1.x,
            v1.y
        );
        // The sane depth is the load-bearing signal: w must be ~ -z_eye = 120,
        // never the pre-fix ±thousands sign-flipping garbage. (The transposed
        // apply gives v1 a NEGATIVE w=-9.9 -- behind the camera.)
        assert!(
            (v0.w - 120.0).abs() < 0.5,
            "clip-w must be the sane perspective depth ~120, got {}",
            v0.w
        );
    }

    // --- G_DL branch (gsSPBranchList) desync regression -----------------
    //
    // Fails against the pre-fix decoder: a G_DL with the NOPUSH (branch)
    // flag used to recurse into the target and then CONTINUE decoding the
    // parent stream. Because a branch's trailing bytes are not commands
    // (here: raw garbage), the decoder walked into them and every byte
    // became a bogus opcode -- the exact ~14K-junk-skip cascade seen on the
    // real OoT gameplay task. After the fix a branch STOPS the parent stream.

    #[test]
    fn g_dl_branch_does_not_decode_bytes_after_the_branch() {
        // Layout:
        //   0x1000  parent DL: [G_DL NOPUSH -> 0x2000], then GARBAGE, G_ENDDL
        //   0x2000  target DL: [G_VTX(3) @ 0x3000], [G_TRI1 0,1,2], G_ENDDL
        //   0x3000  three vertices
        let mut rdram = vec![0u8; 0x4000];

        // Parent stream at 0x1000.
        // gsSPBranchList: w0 = G_DL<<24 | G_DL_NOPUSH<<16, w1 = target addr.
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_DL as u32) << 24) | (0x01 << 16),
            0x2000,
        );
        // "Garbage" right after the branch that the PRE-FIX decoder would
        // wrongly execute: a second VTX+TRI1 pair drawing a spurious extra
        // triangle. (In the real bug these trailing bytes were zero-fill /
        // an unrelated buffer that cascaded into ~14K junk-opcode skips; a
        // spurious *triangle* is the same "kept decoding after the branch"
        // fault, made observable as a hard count assertion.)
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        wr_cmd(
            &mut rdram,
            0x1010,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);

        // Target stream at 0x2000: load 3 verts, draw 1 triangle, end.
        // G_VTX: n=3 in bits 12-19, end=3 in bits 1-7 -> v0 = end - n = 0.
        wr_cmd(
            &mut rdram,
            0x2000,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        // G_TRI1: three 7-bit slots at bits 17/9/1 -> slots 0,1,2.
        wr_cmd(
            &mut rdram,
            0x2008,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x2010, (G_ENDDL as u32) << 24, 0);

        // Three vertices (raw screen coords; no transform loaded).
        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        // Segment 0 is identity here (addresses are already physical).
        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();

        // Exactly the ONE triangle from the branched-to target -- no extra
        // garbage triangles, and (the real proof) no unrecognized-opcode
        // cascade from decoding the bytes after the branch. Pre-fix this
        // would have walked the 0x1008.. garbage as opcodes.
        assert_eq!(
            tris.len(),
            1,
            "branch must run the target then stop; got {} triangles \
             (pre-fix bug decoded post-branch garbage)",
            tris.len()
        );
        // The triangle carries the three planted vertex colors.
        assert_eq!(tris[0].v[0].r, 255);
        assert_eq!(tris[0].v[1].g, 255);
        assert_eq!(tris[0].v[2].b, 255);
    }

    #[test]
    fn g_dl_call_resumes_parent_after_target() {
        // A CALL (G_DL_PUSH=0) must recurse AND resume the parent: parent
        // draws one tri, calls a sub-DL that draws one tri, then parent draws
        // a third after the call returns -> 3 triangles total.
        let mut rdram = vec![0u8; 0x4000];

        // Shared vertices at 0x3000 (0,1,2).
        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        let vtx = |rd: &mut [u8], off: usize| {
            wr_cmd(
                rd,
                off,
                ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
                0x3000,
            );
        };
        let tri1 = |rd: &mut [u8], off: usize| {
            wr_cmd(rd, off, ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1), 0);
        };

        // Parent at 0x1000: VTX, TRI1, G_DL CALL -> 0x2000, TRI1, ENDDL.
        vtx(&mut rdram, 0x1000);
        tri1(&mut rdram, 0x1008);
        wr_cmd(&mut rdram, 0x1010, (G_DL as u32) << 24, 0x2000); // push=0 -> CALL
        tri1(&mut rdram, 0x1018);
        wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

        // Sub-DL at 0x2000: VTX, TRI1, ENDDL.
        vtx(&mut rdram, 0x2000);
        tri1(&mut rdram, 0x2008);
        wr_cmd(&mut rdram, 0x2010, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(
            tris.len(),
            3,
            "call must resume the parent after the target returns"
        );
    }

    #[test]
    fn g_dl_branch_chain_longer_than_call_stack_decodes_fully() {
        // A chain of 40 tail branches (gsSPBranchList) ending in a DL that
        // draws one triangle. On hardware a branch consumes NO return-stack
        // entry, so any chain length is legal. The pre-fix decoder recursed
        // per branch and counted it against MAX_DL_DEPTH, so a chain longer
        // than the cap silently dropped the tail (this exact "G_DL recursion
        // exceeded" warning fired on real OoT field frames).
        const CHAIN: usize = 40;
        let mut rdram = vec![0u8; 0x8000];

        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        // Links at 0x1000, 0x1010, 0x1020, ... each: [branch -> next], and
        // garbage would follow (nothing does -- a branch never returns).
        for i in 0..CHAIN {
            let at = 0x1000 + i * 0x10;
            let next = (0x1000 + (i + 1) * 0x10) as u32;
            wr_cmd(&mut rdram, at, ((G_DL as u32) << 24) | (0x01 << 16), next);
        }
        // Terminal DL after the last link: VTX, TRI1, ENDDL.
        let end = 0x1000 + CHAIN * 0x10;
        wr_cmd(
            &mut rdram,
            end,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        wr_cmd(
            &mut rdram,
            end + 8,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, end + 16, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(
            tris.len(),
            1,
            "a {CHAIN}-deep branch chain must reach its terminal DL \
             (branches consume no stack entry)"
        );
    }

    #[test]
    #[should_panic(expected = "exceeded the 1048576-command budget")]
    fn g_dl_cyclic_branch_traps_at_the_global_command_budget() {
        // A branch list that branches to ITSELF: hardware would spin
        // forever; the finite host decoder must identify the cycle instead
        // of returning a plausible empty render.
        let mut rdram = vec![0u8; 0x2000];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_DL as u32) << 24) | (0x01 << 16),
            0x1000,
        );
        let _ = decode_display_list_f3dex2(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "F3DEX2 display list is truncated at RDRAM 0x00001000")]
    fn truncated_f3dex2_command_stream_traps_with_pc() {
        let rdram = vec![0u8; 0x1004];
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_TEXRECT is truncated at RDRAM 0x00001008")]
    fn truncated_texture_rectangle_traps_before_losing_command_alignment() {
        let mut rdram = vec![0u8; 0x1008];
        wr_cmd(&mut rdram, 0x1000, (G_TEXRECT as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_VTX reads past RDRAM")]
    fn truncated_vertex_dma_traps_instead_of_partially_updating_the_cache() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (2 << 12) | (2 << 1),
            0x1010,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_VTX encoded end slot 1 and count 2")]
    fn malformed_vertex_cache_range_traps_instead_of_saturating_to_slot_zero() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (2 << 12) | (1 << 1),
            0x1080,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_TRI vertex-cache slots [32, 0, 0]")]
    fn triangle_with_nonexistent_cache_slot_traps_instead_of_disappearing() {
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(&mut rdram, 0x1000, ((G_TRI1 as u32) << 24) | (32 << 17), 0);
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_TEXTURE enables tile 0 but no initialized TMEM image")]
    fn enabled_texture_without_live_tmem_traps_instead_of_substituting_white() {
        const DL: usize = 0x1000;
        const VERTICES: usize = 0x1080;
        let mut rdram = vec![0u8; 0x1100];
        for index in 0..3 {
            wr_vtx(
                &mut rdram,
                VERTICES + index * VTX_STRIDE,
                index as i16,
                index as i16,
                0,
                [255; 4],
            );
        }
        wr_cmd(
            &mut rdram,
            DL,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            VERTICES as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_TEXTURE as u32) << 24) | 2,
            0xffff_ffff,
        );
        wr_cmd(
            &mut rdram,
            DL + 16,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, DL as u32);
    }

    #[test]
    #[should_panic(expected = "G_MOVEMEM G_MV_VIEWPORT reads past RDRAM")]
    fn truncated_viewport_dma_traps_instead_of_retaining_stale_state() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEMEM as u32) << 24) | (1 << 19) | G_MV_VIEWPORT as u32,
            0x1018,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_VTX with an active matrix requires G_MOVEMEM G_MV_VIEWPORT")]
    fn transformed_vertex_without_viewport_traps_instead_of_inventing_screen_mapping() {
        const DL: usize = 0x1000;
        const MATRIX: usize = 0x1080;
        const VERTEX: usize = 0x10c0;
        let mut rdram = vec![0u8; 0x1100];
        wr_mtx(&mut rdram, MATRIX, identity());
        wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [255; 4]);
        wr_cmd(
            &mut rdram,
            DL,
            ((G_MTX as u32) << 24) | (7 << 19) | 0x07,
            MATRIX as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
            VERTEX as u32,
        );
        wr_cmd(&mut rdram, DL + 16, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_ops(&rdram, DL as u32);
    }

    #[test]
    #[should_panic(expected = "G_MOVEMEM G_MV_LIGHT reads past RDRAM")]
    fn truncated_light_dma_traps_instead_of_retaining_stale_state() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEMEM as u32) << 24) | (1 << 19) | (6 << 8) | G_MV_LIGHT as u32,
            0x1018,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "G_MTX reads past RDRAM")]
    fn truncated_matrix_dma_traps_instead_of_retaining_the_previous_transform() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MTX as u32) << 24) | (7 << 19) | 0x03,
            0x1010,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "malformed G_SETOTHERMODE_H range")]
    fn malformed_other_mode_range_traps_instead_of_retaining_stale_bits() {
        let mut rdram = vec![0u8; 0x1010];
        // low byte stores len-1; 0x20 therefore requests an impossible
        // 33-bit update of a 32-bit other-mode word.
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_SETOTHERMODE_H as u32) << 24) | 0x20,
            0,
        );
        let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
    }

    #[test]
    fn g_texrect_consumes_two_words_and_does_not_desync() {
        // A G_TEXRECT (0xE4) is a 16-byte command. If the decoder advances
        // only 8 bytes it reads the coord word as a bogus opcode. Here the
        // texrect's second word is crafted to look like a G_VTX opcode
        // (0x01..) that, if wrongly decoded, would load a spurious vertex.
        // A correct 16-byte skip walks straight to the real G_TRI1.
        let mut rdram = vec![0u8; 0x4000];

        wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

        // VTX (3 verts).
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        // G_TEXRECT word 0 + word 1. The SECOND 8-byte word starts with 0x01
        // (a G_VTX opcode byte) to catch an under-advance.
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_TEXRECT as u32) << 24) | 0x00abcdef,
            0x12345678,
        );
        wr_cmd(&mut rdram, 0x1010, 0x0100_4008, 0x0100_1c00); // texrect 2nd word

        // Real G_TRI1 after the full 16-byte texrect.
        wr_cmd(
            &mut rdram,
            0x1018,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(
            tris.len(),
            1,
            "texrect must consume both words so the following G_TRI1 is \
             decoded at the right offset"
        );
    }

    #[test]
    fn texture_rectangle_preserves_signed_fixed_point_and_cycle_state() {
        let mut rdram = vec![0u8; 0x2000];
        // Copy cycle plus threshold alpha compare in one full other-mode
        // command. Blend alpha 0x80 becomes the threshold snapshot.
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_RDPSETOTHERMODE as u32) << 24) | (2 << 20),
            1,
        );
        wr_cmd(
            &mut rdram,
            0x1008,
            (G_SETBLENDCOLOR as u32) << 24,
            0x0000_0080,
        );
        wr_cmd(
            &mut rdram,
            0x1010,
            ((G_TEXRECT as u32) << 24) | ((4 * 4) << 12) | (5 * 4),
            (3 << 24) | ((1 * 4) << 12) | (2 * 4),
        );
        // s=-1.5 (S10.5), t=2.25; dsdx=4.0 and dtdy=0.5 (S5.10).
        wr_cmd(&mut rdram, 0x1018, 0xffd0_0048, 0x1000_0200);
        wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

        let ops = decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
            panic!("expected ordered texture rectangle, got {:?}", ops[0]);
        };
        assert_eq!((rectangle.ulx, rectangle.uly), (1.0, 2.0));
        assert_eq!((rectangle.lrx, rectangle.lry), (4.0, 5.0));
        assert_eq!(rectangle.tile, 3);
        assert_eq!((rectangle.s, rectangle.t), (-1.5, 2.25));
        assert_eq!((rectangle.dsdx, rectangle.dtdy), (4096, 512));
        assert_eq!(rectangle.other_mode.cycle_type(), CycleType::Copy);
        assert_eq!(
            rectangle.other_mode.alpha_compare(),
            AlphaCompare::Threshold
        );
        assert_eq!(rectangle.other_mode.blend_color_alpha, 0x80);
        assert!(!rectangle.flip);
        assert!(rectangle.texture.is_none());
        assert!(rectangle.texture1.is_none());
    }

    #[test]
    fn admitted_geometry_families_decode_all_public_texture_rectangle_continuations() {
        let families = [
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
        ];
        let signed_boundary_vectors = [
            (0x8000_8000, 0x8000_8000),
            (0xffff_0001, 0xffff_0001),
            (0x0000_0000, 0x0000_0000),
            (0x7fff_7fff, 0x7fff_7fff),
        ];

        for (family_index, family) in families.into_iter().enumerate() {
            let text = vec![family_index as u8 + 1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
            let mut catalog = GeometryUcodeCatalog::default();
            catalog.admit_text_for(family, &text);
            let selected_family = catalog
                .require_text(&text)
                .expect("the exact admitted text must select its wire family");
            assert_eq!(selected_family, family);

            let modern = matches!(
                family,
                GeometryWireFamily::F3dex2
                    | GeometryWireFamily::F3dex2NoN
                    | GeometryWireFamily::F3dex2Rej
                    | GeometryWireFamily::F3dlx2Rej
                    | GeometryWireFamily::L3dex2
            );
            let (half_1, half_2, enddl) = if modern {
                (G_RDPHALF_1, G_RDPHALF_2, G_ENDDL)
            } else {
                (LEGACY_G_RDPHALF_1, LEGACY_G_RDPHALF_2, L3DEX_G_ENDDL)
            };

            for opcode in [G_TEXRECT, G_TEXRECTFLIP] {
                for (coords, gradients) in signed_boundary_vectors {
                    let decode_form = |enveloped: bool| {
                        let mut rdram = vec![0u8; 0x1100];
                        wr_cmd(
                            &mut rdram,
                            0x1000,
                            (u32::from(opcode) << 24) | (16 << 12) | 20,
                            (3 << 24) | (4 << 12) | 8,
                        );
                        if enveloped {
                            wr_cmd(&mut rdram, 0x1008, u32::from(half_1) << 24, coords);
                            wr_cmd(&mut rdram, 0x1010, u32::from(half_2) << 24, gradients);
                            wr_cmd(&mut rdram, 0x1018, u32::from(enddl) << 24, 0);
                        } else {
                            wr_cmd(&mut rdram, 0x1008, coords, gradients);
                            wr_cmd(&mut rdram, 0x1010, u32::from(enddl) << 24, 0);
                        }
                        decode_geometry_fixture(&rdram, selected_family)
                    };

                    let direct = decode_form(false);
                    let enveloped = decode_form(true);
                    for operations in [&direct, &enveloped] {
                        assert_eq!(
                            operations.len(),
                            1,
                            "family={family:?} opcode={opcode:#04x}"
                        );
                    }
                    let (RenderOp::TextureRectangle(direct), RenderOp::TextureRectangle(enveloped)) =
                        (&direct[0], &enveloped[0])
                    else {
                        panic!("texture-rectangle vector must emit a typed rectangle");
                    };
                    assert_eq!(
                        (
                            direct.ulx,
                            direct.uly,
                            direct.lrx,
                            direct.lry,
                            direct.tile,
                            direct.s,
                            direct.t,
                            direct.dsdx,
                            direct.dtdy,
                            direct.flip,
                        ),
                        (
                            enveloped.ulx,
                            enveloped.uly,
                            enveloped.lrx,
                            enveloped.lry,
                            enveloped.tile,
                            enveloped.s,
                            enveloped.t,
                            enveloped.dsdx,
                            enveloped.dtdy,
                            enveloped.flip,
                        ),
                        "family={family:?} opcode={opcode:#04x} coords={coords:#010x} gradients={gradients:#010x}"
                    );
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "wrong-family G_RDPHALF_1 opcode 0xb4")]
    fn modern_texture_rectangle_rejects_legacy_continuation_envelope() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, u32::from(G_TEXRECT) << 24, 0);
        wr_cmd(&mut rdram, 0x1008, u32::from(LEGACY_G_RDPHALF_1) << 24, 0);
        wr_cmd(&mut rdram, 0x1010, u32::from(LEGACY_G_RDPHALF_2) << 24, 0);
        let _ = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex2);
    }

    #[test]
    #[should_panic(expected = "G_RDPHALF_2 continuation must be opcode 0xb3")]
    fn legacy_texture_rectangle_rejects_malformed_second_continuation() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(&mut rdram, 0x1000, u32::from(G_TEXRECT) << 24, 0);
        wr_cmd(&mut rdram, 0x1008, u32::from(LEGACY_G_RDPHALF_1) << 24, 0);
        wr_cmd(&mut rdram, 0x1010, u32::from(G_RDPHALF_2) << 24, 0);
        let _ = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex);
    }

    #[test]
    #[should_panic(expected = "continuation envelope is truncated")]
    fn legacy_texture_rectangle_rejects_truncated_continuation_envelope() {
        let mut rdram = vec![0u8; 0x1010];
        wr_cmd(&mut rdram, 0x1000, u32::from(G_TEXRECT) << 24, 0);
        wr_cmd(&mut rdram, 0x1008, u32::from(LEGACY_G_RDPHALF_1) << 24, 0);
        let _ = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex);
    }

    #[test]
    fn display_list_rgba32_load_uses_public_sixteen_bit_load_descriptor() {
        const DL: usize = 0x1000;
        const IMAGE: usize = 0x1800;
        let mut rdram = vec![0u8; 0x2000];
        for (index, value) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
            .into_iter()
            .enumerate()
        {
            wr_u8(&mut rdram, IMAGE + index, value);
        }
        let mut offset = DL;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_SETTIMG as u32) << 24)
                | ((G_IM_FMT_RGBA as u32) << 21)
                | ((G_IM_SIZ_32B as u32) << 19)
                | 1,
            IMAGE as u32,
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_SETTILE as u32) << 24)
                | ((G_IM_FMT_RGBA as u32) << 21)
                | ((G_IM_SIZ_16B as u32) << 19)
                | (1 << 9),
            7 << 24,
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            (G_LOADTILE as u32) << 24,
            (7 << 24) | (4 << 12),
        );
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_SETTILE as u32) << 24)
                | ((G_IM_FMT_RGBA as u32) << 21)
                | ((G_IM_SIZ_32B as u32) << 19)
                | (1 << 9),
            0,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_SETTILESIZE as u32) << 24, 4 << 12);
        offset += 8;
        wr_cmd(
            &mut rdram,
            offset,
            ((G_TEXRECT as u32) << 24) | (8 << 12) | 4,
            0,
        );
        offset += 8;
        wr_cmd(&mut rdram, offset, 0, 0x0400_0400);
        offset += 8;
        wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

        let ops = decode_display_list_f3dex2_ops(&rdram, DL as u32).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
            panic!("expected texture rectangle, got {:?}", ops[0]);
        };
        let texture = rectangle.texture.as_ref().expect("RGBA32 tile must bind");
        assert_eq!(texture.sample(0.0, 0.0), [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(texture.sample(1.0, 0.0), [0x50, 0x60, 0x70, 0x80]);
    }

    // --- Viewport mapping (priority 1) ----------------------------------

    #[test]
    fn read_viewport_divides_quarter_pixel_encoding_by_four() {
        // OoT's real full-screen viewport: vscale (640,480,z), vtrans same,
        // in the ×4 "quarter-pixel" encoding -> 160/120 px after ÷4 (§3.5).
        let mut rdram = vec![0u8; 64];
        let addr = 0x10;
        wr_i16(&mut rdram, addr, 640); // vscale.x
        wr_i16(&mut rdram, addr + 2, 480); // vscale.y
        wr_i16(&mut rdram, addr + 4, 511); // vscale.z (~127.75 depth)
        wr_i16(&mut rdram, addr + 8, 640); // vtrans.x
        wr_i16(&mut rdram, addr + 10, 480); // vtrans.y
        wr_i16(&mut rdram, addr + 12, 511); // vtrans.z
        let vp = read_viewport(&rdram, addr).expect("viewport in bounds");
        assert_eq!(vp.sx, 160.0);
        assert_eq!(vp.sy, 120.0);
        assert_eq!(vp.tx, 160.0);
        assert_eq!(vp.ty, 120.0);
        assert_eq!(vp.sz, 127.75);
    }

    #[test]
    fn viewport_maps_known_ndc_points_to_known_pixels() {
        // A 320×240 centered viewport (sx=160, tx=160, sy=120, ty=120).
        // Map the NDC corners the way `project_vertex` does (with the Y-flip).
        let vp = Viewport {
            sx: 160.0,
            sy: 120.0,
            sz: 127.75,
            tx: 160.0,
            ty: 120.0,
            tz: 127.75,
        };
        // NDC origin (0,0) -> screen center (160,120).
        let map = |nx: f32, ny: f32| (nx * vp.sx + vp.tx, -ny * vp.sy + vp.ty);
        assert_eq!(map(0.0, 0.0), (160.0, 120.0));
        // NDC (-1,+1) is top-left on screen after the Y-flip: (0, 0).
        assert_eq!(map(-1.0, 1.0), (0.0, 0.0));
        // NDC (+1,-1) is bottom-right: (320, 240).
        assert_eq!(map(1.0, -1.0), (320.0, 240.0));
    }

    // --- Culling (priority 2) -------------------------------------------

    #[test]
    fn cull_mode_from_geometry_mode_bits() {
        assert_eq!(cull_mode_from(0), CullMode::None);
        assert_eq!(cull_mode_from(G_CULL_BACK), CullMode::Back);
        assert_eq!(cull_mode_from(G_CULL_FRONT), CullMode::Front);
        assert_eq!(cull_mode_from(G_CULL_FRONT | G_CULL_BACK), CullMode::Both);
        // Unrelated bits (e.g. G_SHADE=0x4, G_ZBUFFER=0x1) don't cull.
        assert_eq!(cull_mode_from(0x0000_0005), CullMode::None);
    }

    // --- Vertex lighting (priority 3) -----------------------------------

    /// Write a byte at logical offset `off` through the recomp `^3` swizzle
    /// (mirrors `read_u8`'s memory model), so tests plant Light_t/Vtx bytes
    /// the way a real DMA would.
    fn wr_u8(rdram: &mut [u8], off: usize, v: u8) {
        rdram[off ^ 3] = v;
    }

    /// A `DecodeState` with identity modelview and no MVP -- the minimal
    /// harness for exercising the light math directly.
    fn lit_state() -> DecodeState {
        DecodeState {
            vtx_cache: [Vertex::default(); 64],
            ops: Vec::new(),
            segments: [0u32; 16],
            mvp: None,
            pending_forced_mvp: None,
            proj: None,
            modelview: identity(),
            mv_stack: Vec::new(),
            viewport: None,
            scissor: None,
            geometry_mode: 0,
            other_mode: OtherMode::default(),
            combiner: CombinerState::default(),
            blend_color: [0; 4],
            fog_color: [0; 4],
            fill_color: 0,
            rdp_half_1: None,
            dl_depth: 0,
            cmds_decoded: 0,
            tex: TexState::default(),
            lights: LightState::default(),
            look_at: LookAtState::default(),
            fog: FogFactor::default(),
            persp_normalize: PerspectiveNormalize::default(),
            clip_ratio: ClipRatio::default(),
            unsupported_ucode_reload: None,
        }
    }

    #[test]
    fn load_ucode_resets_rsp_state_but_preserves_independent_rdp_state() {
        let mut state = lit_state();
        state.geometry_mode = G_LIGHTING | G_CULL_BACK;
        state.proj = Some(identity());
        state.modelview = identity();
        state.mvp = Some(identity());
        state.pending_forced_mvp = Some(identity());
        state.mv_stack.push(identity());
        state.segments[3] = 0x0012_3000;
        state.viewport = Some(Viewport {
            sx: 160.0,
            sy: 120.0,
            sz: 127.75,
            tx: 160.0,
            ty: 120.0,
            tz: 127.75,
        });
        state.tex.tex_enabled = true;
        state.tex.tex_tile = 7;
        state.tex.tex_max_level = 5;
        state.tex.tex_scale_s = 1.0;
        state.tex.tex_scale_t = 1.0;
        state.lights.num_dir = 2;
        state.look_at.x = Some([1.0, 0.0, 0.0]);
        state.look_at.y = Some([0.0, 1.0, 0.0]);
        state.persp_normalize = PerspectiveNormalize(Some(0x1234));
        state.clip_ratio = ClipRatio {
            neg_x: 2,
            neg_y: 3,
            pos_x: 4,
            pos_y: 5,
        };
        state.fog = FogFactor {
            multiplier: 7,
            offset: -9,
        };
        state.rdp_half_1 = Some(0x1234_5678);
        state.fog_color = [1, 2, 3, 4];
        state.scissor = Some(ScissorRect {
            ulx: 1.0,
            uly: 2.0,
            lrx: 3.0,
            lry: 4.0,
            field: false,
            keep_odd: false,
        });
        state.other_mode.low = 0x1234;
        state.combiner.primitive = [5, 6, 7, 8];

        reset_rsp_state_from_ucode_load(&mut state);

        assert_eq!(state.geometry_mode, 0);
        assert_eq!(state.proj, Some(identity()));
        assert_eq!(state.modelview, identity());
        assert!(state.mvp.is_none());
        assert!(state.pending_forced_mvp.is_none());
        assert_eq!(state.mv_stack, vec![identity()]);
        assert_eq!(state.segments[3], 0x0012_3000);
        assert!(state.viewport.is_some());
        assert!(!state.tex.tex_enabled);
        assert_eq!(state.tex.tex_tile, 0);
        assert_eq!(state.tex.tex_max_level, 0);
        assert_eq!(state.tex.tex_scale_s, 0.0);
        assert_eq!(state.tex.tex_scale_t, 0.0);
        assert_eq!(state.lights.num_dir, 0);
        assert_eq!(state.look_at, LookAtState::default());
        assert_eq!(
            state.persp_normalize,
            PerspectiveNormalize(Some(0x1234)),
            "public F3DEX2 maintained-state list preserves PerspNormalize"
        );
        assert_eq!(
            state.clip_ratio,
            ClipRatio::default(),
            "clip ratio is absent from the exhaustive F3DEX2 maintained-state list"
        );
        assert_eq!(state.fog, FogFactor::default());
        assert_eq!(state.rdp_half_1, None);
        assert_eq!(state.fog_color, [1, 2, 3, 4]);
        assert!(state.scissor.is_some(), "RDP scissor survives RSP reload");
        assert_eq!(state.other_mode.low, 0x1234);
        assert_eq!(state.combiner.primitive, [5, 6, 7, 8]);
    }

    #[test]
    fn legacy_load_ucode_resets_all_rsp_geometry_state_but_preserves_rdp_state() {
        let mut state = lit_state();
        state.vtx_cache[3].x = 9.0;
        state.segments[3] = 0x0012_3000;
        state.proj = Some(identity());
        state.modelview[3][0] = 5.0;
        state.mvp = Some(identity());
        state.pending_forced_mvp = Some(identity());
        state.mv_stack.push(identity());
        state.viewport = Some(Viewport {
            sx: 160.0,
            sy: 120.0,
            sz: 127.75,
            tx: 160.0,
            ty: 120.0,
            tz: 127.75,
        });
        state.geometry_mode = G_LIGHTING | G_CULL_BACK;
        state.rdp_half_1 = Some(0x1234_5678);
        state.tex.tex_enabled = true;
        state.tex.tex_tile = 7;
        state.tex.tex_max_level = 5;
        state.tex.tex_scale_s = 1.0;
        state.tex.tex_scale_t = 1.0;
        state.lights.num_dir = 2;
        state.look_at.x = Some([1.0, 0.0, 0.0]);
        state.fog = FogFactor {
            multiplier: 7,
            offset: -9,
        };
        state.persp_normalize = PerspectiveNormalize(Some(0x1234));
        state.clip_ratio = ClipRatio {
            neg_x: 2,
            neg_y: 3,
            pos_x: 4,
            pos_y: 5,
        };

        state.tex.timg_addr = 0x0013_0000;
        state.tex.tlut.push([1, 2, 3, 4]);
        state.fog_color = [5, 6, 7, 8];
        state.fill_color = 0x1234_5678;
        state.scissor = Some(ScissorRect {
            ulx: 1.0,
            uly: 2.0,
            lrx: 3.0,
            lry: 4.0,
            field: false,
            keep_odd: false,
        });
        state.other_mode.low = 0x1234;
        state.combiner.primitive = [9, 10, 11, 12];

        reset_legacy_rsp_state_from_ucode_load(&mut state);

        assert_eq!(state.vtx_cache, [Vertex::default(); 64]);
        assert_eq!(state.segments, [0; 16]);
        assert!(state.proj.is_none());
        assert_eq!(state.modelview, identity());
        assert!(state.mvp.is_none());
        assert!(state.pending_forced_mvp.is_none());
        assert!(state.mv_stack.is_empty());
        assert!(state.viewport.is_none());
        assert_eq!(state.geometry_mode, 0);
        assert_eq!(state.rdp_half_1, None);
        assert!(!state.tex.tex_enabled);
        assert_eq!(state.tex.tex_tile, 0);
        assert_eq!(state.tex.tex_max_level, 0);
        assert_eq!(state.tex.tex_scale_s, 0.0);
        assert_eq!(state.tex.tex_scale_t, 0.0);
        assert_eq!(state.lights.num_dir, 0);
        assert_eq!(state.look_at, LookAtState::default());
        assert_eq!(state.fog, FogFactor::default());
        assert_eq!(state.persp_normalize, PerspectiveNormalize::default());
        assert_eq!(state.clip_ratio, ClipRatio::default());

        assert_eq!(state.tex.timg_addr, 0x0013_0000);
        assert_eq!(state.tex.tlut, vec![[1, 2, 3, 4]]);
        assert_eq!(state.fog_color, [5, 6, 7, 8]);
        assert_eq!(state.fill_color, 0x1234_5678);
        assert!(state.scissor.is_some());
        assert_eq!(state.other_mode.low, 0x1234);
        assert_eq!(state.combiner.primitive, [9, 10, 11, 12]);
    }

    #[test]
    #[should_panic(
        expected = "F3DEX/L3DEX G_LOAD_UCODE inside a called display list resets link state and cannot return"
    )]
    fn legacy_load_ucode_in_called_list_traps_before_resetting_link_state() {
        let mut state = lit_state();
        state.dl_depth = 1;
        reset_legacy_rsp_state_from_ucode_load(&mut state);
    }

    #[test]
    fn rdp_registers_tiles_and_tmem_survive_f3dex2_task_boundaries() {
        const LOAD_DL: usize = 0x1000;
        const DRAW_DL: usize = 0x1100;
        const VERTICES: usize = 0x1200;
        const IMAGE: usize = 0x1300;
        let mut rdram = vec![0u8; 0x1400];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let catalog = F3dex2UcodeCatalog::default();
        let mut rdp_state = RdpDecodeState::default();

        // First task: load one opaque-red RGBA16 texel into physical TMEM,
        // configure render tile 0, and program a constant RDP register.
        wr_u8(&mut rdram, IMAGE, 0xf8);
        wr_u8(&mut rdram, IMAGE + 1, 0x01);
        let mut pc = LOAD_DL;
        wr_cmd(
            &mut rdram,
            pc,
            ((G_SETTIMG as u32) << 24)
                | ((G_IM_FMT_RGBA as u32) << 21)
                | ((G_IM_SIZ_16B as u32) << 19),
            IMAGE as u32,
        );
        pc += 8;
        wr_cmd(
            &mut rdram,
            pc,
            ((G_SETTILE as u32) << 24)
                | ((G_IM_FMT_RGBA as u32) << 21)
                | ((G_IM_SIZ_16B as u32) << 19)
                | (1 << 9),
            7 << 24,
        );
        pc += 8;
        wr_cmd(&mut rdram, pc, (G_LOADTILE as u32) << 24, 7 << 24);
        pc += 8;
        wr_cmd(
            &mut rdram,
            pc,
            ((G_SETTILE as u32) << 24)
                | ((G_IM_FMT_RGBA as u32) << 21)
                | ((G_IM_SIZ_16B as u32) << 19)
                | (1 << 9),
            0,
        );
        pc += 8;
        wr_cmd(&mut rdram, pc, (G_SETTILESIZE as u32) << 24, 0);
        pc += 8;
        wr_cmd(&mut rdram, pc, (G_SETPRIMCOLOR as u32) << 24, 0x12_34_56_78);
        pc += 8;
        wr_cmd(&mut rdram, pc, (G_ENDDL as u32) << 24, 0);

        let load_ops = execute_display_list_f3dex2_ops_admitted_with_rdp_state(
            &mut rdram,
            &mut rsp_memory,
            LOAD_DL as u32,
            &catalog,
            &mut rdp_state,
        )
        .unwrap();
        assert!(load_ops.is_empty());
        assert!(!rdp_state.tex.tex_enabled, "G_TEXTURE is RSP state");

        // Second task: only its RSP-owned G_TEXTURE/vertex state is rebuilt.
        // No RDP texture/register setup is repeated.
        wr_vtx(&mut rdram, VERTICES, 1, 1, 0, [255; 4]);
        wr_vtx(&mut rdram, VERTICES + VTX_STRIDE, 5, 1, 0, [255; 4]);
        wr_vtx(&mut rdram, VERTICES + 2 * VTX_STRIDE, 1, 5, 0, [255; 4]);
        wr_cmd(
            &mut rdram,
            DRAW_DL,
            ((G_TEXTURE as u32) << 24) | 2,
            0xffff_ffff,
        );
        wr_cmd(
            &mut rdram,
            DRAW_DL + 8,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            VERTICES as u32,
        );
        wr_cmd(
            &mut rdram,
            DRAW_DL + 16,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, DRAW_DL + 24, (G_ENDDL as u32) << 24, 0);

        let draw_ops = execute_display_list_f3dex2_ops_admitted_with_rdp_state(
            &mut rdram,
            &mut rsp_memory,
            DRAW_DL as u32,
            &catalog,
            &mut rdp_state,
        )
        .unwrap();
        let RenderOp::Triangle(triangle) = &draw_ops[0] else {
            panic!("expected one triangle, got {:?}", draw_ops[0]);
        };
        assert_eq!(triangle.combiner.primitive, [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(
            triangle
                .texture
                .as_ref()
                .expect("task-two G_TEXTURE must bind task-one TMEM")
                .sample(0.0, 0.0),
            [255, 0, 0, 255]
        );
        assert!(!rdp_state.tex.tex_enabled, "task commit clears RSP state");
    }

    #[test]
    fn num_lights_from_moveword_divides_by_24() {
        // gsSPNumLights writes NUML(n) = n*24; num_dir = data/24.
        let mut st = lit_state();
        // 2 directional lights: data = 48.
        st.lights.num_dir = (48u32 / 24) as usize;
        assert_eq!(st.lights.num_dir, 2);
    }

    #[test]
    fn moveword_light_color_updates_directional_and_ambient_slots() {
        let mut rdram = vec![0u8; 0x1100];
        let mut pc = 0x1000;
        // One directional light makes slot 1 the ambient light.
        wr_cmd(
            &mut rdram,
            pc,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_NUMLIGHT as u32) << 16),
            24,
        );
        pc += 8;
        // Public gSPLightColor writes both color copies. Slot 0 uses offsets
        // 0/4 and slot 1 uses 24/28. Alpha is ignored.
        for (offset, color) in [
            (0u16, 0x2040_60ff),
            (4, 0x2040_60ff),
            (24, 0x80a0_c000),
            (28, 0x80a0_c000),
        ] {
            wr_cmd(
                &mut rdram,
                pc,
                ((G_MOVEWORD as u32) << 24) | ((G_MW_LIGHTCOL as u32) << 16) | u32::from(offset),
                color,
            );
            pc += 8;
        }
        wr_cmd(&mut rdram, pc, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
        assert_eq!(state.lights.num_dir, 1);
        assert_eq!(
            state.lights.dir[0].col,
            [32.0 / 255.0, 64.0 / 255.0, 96.0 / 255.0]
        );
        assert_eq!(state.lights.dir[0].dir, [0.0; 3]);
        assert_eq!(
            state.lights.ambient,
            [128.0 / 255.0, 160.0 / 255.0, 192.0 / 255.0]
        );
    }

    #[test]
    #[should_panic(expected = "G_MOVEWORD G_MW_LIGHTCOL offset")]
    fn malformed_moveword_light_color_offset_traps_by_name() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_LIGHTCOL as u32) << 16) | 8,
            0x1122_3344,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
    }

    #[test]
    fn moveword_fog_decodes_signed_factor_halfwords() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_FOG as u32) << 16),
            ((-128i16 as u16 as u32) << 16) | 300,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
        assert_eq!(
            state.fog,
            FogFactor {
                multiplier: -128,
                offset: 300,
            }
        );
    }

    #[test]
    fn moveword_perspective_normalize_retains_public_u16_scale() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_PERSPNORM as u32) << 16),
            0x0000_3456,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
        assert_eq!(state.persp_normalize, PerspectiveNormalize(Some(0x3456)));
    }

    #[test]
    fn moveword_clip_ratio_decodes_all_four_public_destinations() {
        let mut rdram = vec![0u8; 0x1040];
        let mut pc = 0x1000;
        for (offset, value) in [
            (G_MWO_CLIP_RNX, 5),
            (G_MWO_CLIP_RNY, 5),
            (G_MWO_CLIP_RPX, (-5i16 as u16) as u32),
            (G_MWO_CLIP_RPY, (-5i16 as u16) as u32),
        ] {
            wr_cmd(
                &mut rdram,
                pc,
                ((G_MOVEWORD as u32) << 24) | ((G_MW_CLIP as u32) << 16) | u32::from(offset),
                value,
            );
            pc += 8;
        }
        wr_cmd(&mut rdram, pc, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
        assert_eq!(
            state.clip_ratio,
            ClipRatio {
                neg_x: 5,
                neg_y: 5,
                pos_x: 5,
                pos_y: 5,
            }
        );
    }

    #[test]
    #[should_panic(expected = "is not FRUSTRATIO_1..6")]
    fn moveword_clip_ratio_rejects_non_public_value() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_CLIP as u32) << 16) | u32::from(G_MWO_CLIP_RNX),
            7,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
    }

    #[test]
    fn nonzero_perspective_normalize_is_neutral_in_float_reference_divide() {
        let mut state = lit_state();
        state.mvp = Some(identity());
        state.viewport = Some(centered_viewport());
        state.persp_normalize = PerspectiveNormalize(Some(1));
        let smallest = project_vertex(&state, 1.0, -1.0, 0.5);
        state.persp_normalize = PerspectiveNormalize(Some(u16::MAX));
        let largest = project_vertex(&state, 1.0, -1.0, 0.5);
        assert_eq!(smallest, largest);
    }

    #[test]
    fn zero_perspective_normalize_rejects_triangle_and_line_geometry() {
        const DL: usize = 0x1000;
        const VERTICES: usize = 0x1080;
        let mut rdram = vec![0u8; 0x1100];
        wr_vtx(&mut rdram, VERTICES, 0, 0, 0, [255, 0, 0, 255]);
        wr_vtx(
            &mut rdram,
            VERTICES + VTX_STRIDE,
            10,
            0,
            0,
            [0, 255, 0, 255],
        );
        wr_vtx(
            &mut rdram,
            VERTICES + 2 * VTX_STRIDE,
            0,
            10,
            0,
            [0, 0, 255, 255],
        );
        wr_cmd(
            &mut rdram,
            DL,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_PERSPNORM as u32) << 16),
            0,
        );
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            VERTICES as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 16,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        wr_cmd(&mut rdram, DL + 24, ((G_LINE3D as u32) << 24) | (2 << 8), 0);
        wr_cmd(&mut rdram, DL + 32, (G_ENDDL as u32) << 24, 0);

        assert!(
            decode_display_list_f3dex2_ops(&rdram, DL as u32)
                .unwrap()
                .iter()
                .all(|op| !matches!(op, RenderOp::Triangle(_) | RenderOp::Line(_))),
            "zero perspective-normalization scale must not produce triangle or line geometry"
        );
    }

    #[test]
    #[should_panic(expected = "scale must be a public u16 value")]
    fn perspective_normalize_rejects_non_public_high_bits() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_PERSPNORM as u32) << 16),
            0x0001_0000,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
    }

    #[test]
    fn fog_geometry_mode_replaces_vertex_alpha_from_projected_depth() {
        let mut rdram = vec![0u8; 0x1100];
        let base = 0x1000;
        wr_vtx(&mut rdram, base, 0, 0, -1, [1, 2, 3, 17]);
        wr_vtx(&mut rdram, base + VTX_STRIDE, 0, 0, 0, [1, 2, 3, 17]);
        wr_vtx(&mut rdram, base + 2 * VTX_STRIDE, 0, 0, 1, [1, 2, 3, 17]);
        let mut state = lit_state();
        state.mvp = Some(identity());
        state.viewport = Some(centered_viewport());
        state.geometry_mode = G_FOG;
        state.fog = FogFactor {
            multiplier: 128,
            offset: 128,
        };

        load_vertices(
            &rdram,
            &mut state,
            base as u32,
            3,
            0,
            GeometryWireFamily::F3dex2,
        );
        assert_eq!(state.vtx_cache[0].a, 0);
        assert_eq!(state.vtx_cache[1].a, 128);
        assert_eq!(state.vtx_cache[2].a, 255);
        assert_eq!(state.vtx_cache[1].r, 1);
    }

    #[test]
    fn movemem_light_1_maps_to_directional_slot_zero() {
        // Fail-against-bug wire evidence: gSPLight(LIGHT_1) encodes
        // (1*24 + 24)/8 = 6. The old `ofs/3 - 1` mapping returned slot 1,
        // leaving the real first directional light (slot 0) black/stale and
        // misclassifying LIGHT_1 as ambient when num_dir == 1.
        assert_eq!(light_slot_from_movemem_offset(6), Some(0));
        // LIGHT_2 is the ambient slot when one directional light is active.
        assert_eq!(light_slot_from_movemem_offset(9), Some(1));
        // Offsets for the two look-at vectors are not light slots.
        assert_eq!(light_slot_from_movemem_offset(0), None);
        assert_eq!(light_slot_from_movemem_offset(3), None);
    }

    #[test]
    fn force_matrix_compound_replaces_mvp_without_mutating_matrix_stacks() {
        const DL: usize = 0x1000;
        const MATRIX: usize = 0x1100;
        const VERTEX: usize = 0x1180;
        const VIEWPORT: usize = 0x11f0;
        let mut rdram = vec![0u8; 0x1200];
        wr_mtx(&mut rdram, MATRIX, identity());
        wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [1, 2, 3, 255]);
        wr_centered_viewport(&mut rdram, VIEWPORT);
        wr_cmd(&mut rdram, DL, movemem_viewport_word(), VIEWPORT as u32);
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_MOVEMEM as u32) << 24) | (7 << 19) | G_MV_MATRIX as u32,
            MATRIX as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 16,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_FORCEMTX as u32) << 16),
            0x0001_0000,
        );
        wr_cmd(
            &mut rdram,
            DL + 24,
            ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
            VERTEX as u32,
        );
        wr_cmd(&mut rdram, DL + 32, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
        assert_eq!(state.modelview, identity());
        assert!(state.proj.is_none());
        assert_eq!(state.mvp, Some(identity()));
        assert!(state.pending_forced_mvp.is_none());
        assert_eq!((state.vtx_cache[0].x, state.vtx_cache[0].y), (160.0, 120.0));
    }

    #[test]
    fn ordinary_matrix_command_supersedes_force_matrix_override() {
        const DL: usize = 0x1000;
        const FORCED: usize = 0x1100;
        const PROJECTION: usize = 0x1140;
        const VERTEX: usize = 0x1180;
        const VIEWPORT: usize = 0x11f0;
        let mut rdram = vec![0u8; 0x1200];
        let mut translated = identity();
        translated[3][0] = 0.25;
        wr_mtx(&mut rdram, FORCED, translated);
        wr_mtx(&mut rdram, PROJECTION, identity());
        wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [1, 2, 3, 255]);
        wr_centered_viewport(&mut rdram, VIEWPORT);
        wr_cmd(&mut rdram, DL, movemem_viewport_word(), VIEWPORT as u32);
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_MOVEMEM as u32) << 24) | (7 << 19) | G_MV_MATRIX as u32,
            FORCED as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 16,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_FORCEMTX as u32) << 16),
            0x0001_0000,
        );
        // Public params PROJECTION|LOAD|NOPUSH = 0x06; F3DEX2 wire XORs the
        // push bit, so the low command byte is 0x07.
        wr_cmd(
            &mut rdram,
            DL + 24,
            ((G_MTX as u32) << 24) | (7 << 19) | 0x07,
            PROJECTION as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 32,
            ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
            VERTEX as u32,
        );
        wr_cmd(&mut rdram, DL + 40, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
        assert_eq!(state.mvp, Some(identity()));
        assert_eq!(state.vtx_cache[0].x, 160.0);
    }

    #[test]
    fn modelview_only_matrix_uses_identity_projection() {
        const DL: usize = 0x1000;
        const MODELVIEW: usize = 0x1100;
        const VERTEX: usize = 0x1180;
        const VIEWPORT: usize = 0x11f0;
        let mut rdram = vec![0u8; 0x1200];
        let mut translated = identity();
        translated[3][0] = 0.25;
        wr_mtx(&mut rdram, MODELVIEW, translated);
        wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [1, 2, 3, 255]);
        wr_centered_viewport(&mut rdram, VIEWPORT);
        // MODELVIEW|LOAD|NOPUSH = 0x02, XORed with the F3DEX2 push bit on
        // the wire to 0x03.
        wr_cmd(&mut rdram, DL, movemem_viewport_word(), VIEWPORT as u32);
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_MTX as u32) << 24) | (7 << 19) | 0x03,
            MODELVIEW as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 16,
            ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
            VERTEX as u32,
        );
        wr_cmd(&mut rdram, DL + 24, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
        assert!(state.proj.is_none());
        assert_eq!(state.mvp, Some(translated));
        assert_eq!((state.vtx_cache[0].x, state.vtx_cache[0].y), (200.0, 120.0));
    }

    #[test]
    #[should_panic(expected = "requires a preceding G_MOVEMEM G_MV_MATRIX")]
    fn force_matrix_marker_without_dma_traps_by_both_command_names() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_FORCEMTX as u32) << 16),
            0x0001_0000,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
    }

    #[test]
    #[should_panic(expected = "must carry one 64-byte Mtx")]
    fn force_matrix_dma_rejects_non_public_length_by_name() {
        let mut rdram = vec![0u8; 0x1100];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_MOVEMEM as u32) << 24) | (6 << 19) | G_MV_MATRIX as u32,
            0x1080,
        );
        wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
        let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
    }

    #[test]
    fn movemem_look_at_decodes_both_public_screen_axes() {
        const DL: usize = 0x1000;
        const LOOK_X: usize = 0x1080;
        const LOOK_Y: usize = 0x10a0;
        let mut rdram = vec![0u8; 0x1100];
        wr_u8(&mut rdram, LOOK_X + 8, 127);
        wr_u8(&mut rdram, LOOK_X + 9, 0);
        wr_u8(&mut rdram, LOOK_X + 10, 0x81); // -127
        wr_u8(&mut rdram, LOOK_Y + 8, 0);
        wr_u8(&mut rdram, LOOK_Y + 9, 127);
        wr_u8(&mut rdram, LOOK_Y + 10, 0);

        // gSPLookAtX/Y share G_MV_LIGHT and select public offsets 0*24 and
        // 1*24. The wire stores those destinations divided by eight.
        wr_cmd(
            &mut rdram,
            DL,
            ((G_MOVEMEM as u32) << 24) | (1 << 19) | G_MV_LIGHT as u32,
            LOOK_X as u32,
        );
        wr_cmd(
            &mut rdram,
            DL + 8,
            ((G_MOVEMEM as u32) << 24) | (1 << 19) | (3 << 8) | G_MV_LIGHT as u32,
            LOOK_Y as u32,
        );
        wr_cmd(&mut rdram, DL + 16, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
        assert_eq!(state.look_at.x, Some([1.0, 0.0, -1.0]));
        assert_eq!(state.look_at.y, Some([0.0, 1.0, 0.0]));
    }

    fn texture_generation_state(linear: bool) -> DecodeState {
        let mut state = lit_state();
        state.geometry_mode =
            G_LIGHTING | G_TEXTURE_GEN | if linear { G_TEXTURE_GEN_LINEAR } else { 0 };
        state.look_at = LookAtState {
            x: Some([1.0, 0.0, 0.0]),
            y: Some([0.0, 1.0, 0.0]),
        };
        // Public manual example: gSPTexture(tex_max << 6, ...). A tex_max of
        // 31 must therefore be the generated endpoint at a +1 projection.
        state.tex.tex_scale_s = (31 << 6) as f32 / 65536.0;
        state.tex.tex_scale_t = (31 << 6) as f32 / 65536.0;
        state
    }

    fn assert_texture_coords_close(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() <= 1e-5 && (actual.1 - expected.1).abs() <= 1e-5,
            "texture coordinates differ: actual={actual:?}, expected={expected:?}"
        );
    }

    #[test]
    fn regular_texture_generation_maps_signed_projections_to_scale() {
        let state = texture_generation_state(false);
        assert_eq!(
            generated_texture_coords(&state, [1.0, 0.0, 0.0]),
            (31.0, 15.5)
        );
        assert_eq!(
            generated_texture_coords(&state, [-1.0, 0.0, 0.0]),
            (0.0, 15.5)
        );
        assert_eq!(
            generated_texture_coords(&state, [0.0, 1.0, 0.0]),
            (15.5, 31.0)
        );
    }

    #[test]
    fn linear_texture_generation_maps_inverse_cosine_to_scale() {
        let state = texture_generation_state(true);
        assert_texture_coords_close(
            generated_texture_coords(&state, [1.0, 0.0, 0.0]),
            (0.0, 15.5),
        );
        assert_texture_coords_close(
            generated_texture_coords(&state, [-1.0, 0.0, 0.0]),
            (31.0, 15.5),
        );
        assert_texture_coords_close(
            generated_texture_coords(&state, [0.0, 1.0, 0.0]),
            (15.5, 0.0),
        );
    }

    #[test]
    fn texture_generation_replaces_explicit_vertex_coordinates() {
        let mut rdram = vec![0u8; 64];
        wr_vtx(&mut rdram, 0, 0, 0, 0, [127, 0, 0, 255]);
        wr_i16(&mut rdram, 8, -1234);
        wr_i16(&mut rdram, 10, 2345);
        let mut state = texture_generation_state(false);
        state.lights.ambient = [1.0, 1.0, 1.0];

        load_vertices(&rdram, &mut state, 0, 1, 0, GeometryWireFamily::F3dex2);

        let vertex = state.vtx_cache[0];
        assert_eq!((vertex.s, vertex.t), (31.0, 15.5));
        assert_eq!((vertex.r, vertex.g, vertex.b), (255, 255, 255));
    }

    #[test]
    #[should_panic(expected = "G_TEXTURE_GEN requires G_LIGHTING")]
    fn texture_generation_without_lighting_traps_by_geometry_mode_name() {
        let mut state = texture_generation_state(false);
        state.geometry_mode = G_TEXTURE_GEN;
        let _ = generated_texture_coords(&state, [1.0, 0.0, 0.0]);
    }

    #[test]
    #[should_panic(expected = "gSPLookAtY")]
    fn texture_generation_without_both_look_at_axes_traps_by_command_name() {
        let mut state = texture_generation_state(false);
        state.look_at.y = None;
        let _ = generated_texture_coords(&state, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn load_light_decodes_color_and_signed_direction() {
        // Light_t: col[3] u8 @0..3, dir[3] s8 @8..11. Plant a red light
        // pointing along -Z (dir byte 0x81 == -127 -> ~-1.0 after /127).
        let mut rdram = vec![0u8; 64];
        let addr = 0x10;
        wr_u8(&mut rdram, addr, 255); // col.r
        wr_u8(&mut rdram, addr + 1, 0); // col.g
        wr_u8(&mut rdram, addr + 2, 0); // col.b
        wr_u8(&mut rdram, addr + 8, 0); // dir.x
        wr_u8(&mut rdram, addr + 9, 0); // dir.y
        wr_u8(&mut rdram, addr + 10, 0x81); // dir.z = -127
        let mut st = lit_state();
        st.lights.num_dir = 1; // slot 0 is directional here
        load_light(&rdram, &mut st, addr, 0);
        let l = st.lights.dir[0];
        assert_eq!(l.col, [1.0, 0.0, 0.0]);
        assert!((l.dir[2] - (-127.0 / 127.0)).abs() < 1e-6);
        assert_eq!(l.dir[0], 0.0);
    }

    #[test]
    fn light_vertex_face_on_light_is_full_diffuse_plus_ambient() {
        // One white directional light pointing at the surface normal (+Z),
        // plus a dim gray ambient. A normal facing the light (+Z) gets full
        // N·L=1 -> ambient + light color, clamped.
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.1, 0.1, 0.1];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [0.8, 0.8, 0.8],
        };
        // Normal directly toward the light: N·L = 1.
        let c = light_vertex(&st, [0.0, 0.0, 1.0]);
        // 0.1 + 1.0*0.8 = 0.9 -> 229.
        assert_eq!(c, [229, 229, 229]);
    }

    #[test]
    fn light_vertex_back_face_gets_ambient_only() {
        // A normal facing AWAY from the light (N·L < 0, clamped to 0) is lit
        // by ambient alone -- the diffuse term must not go negative (that
        // was the failure mode a naive dot without a max(.,0) would hit).
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.2, 0.2, 0.2];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [1.0, 1.0, 1.0],
        };
        // Normal pointing away from the +Z light.
        let c = light_vertex(&st, [0.0, 0.0, -1.0]);
        assert_eq!(c, [51, 51, 51]); // 0.2*255 = 51, no negative diffuse.
    }

    #[test]
    fn light_vertex_is_not_the_raw_normal_bytes() {
        // Fail-against-bug: the OLD path read the s8 normal bytes AS a flat
        // color. A normal of (0,0,+1) with a green light must NOT come out as
        // the raw normal-as-color (which would be ~[0,0,255] from cn bytes);
        // it must be the LIT color (green from the light). This is exactly the
        // "rainbow fan" bug: signed normals misread as unsigned color.
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.0, 0.0, 0.0];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [0.0, 1.0, 0.0], // green
        };
        let c = light_vertex(&st, [0.0, 0.0, 1.0]);
        assert_eq!(c, [0, 255, 0]); // green, from the LIGHT -- not the normal.
    }

    #[test]
    fn light_vertex_half_angle_scales_diffuse() {
        // A 45° normal to a +Z light: N·L = cos(45°) ≈ 0.707, so a white
        // light yields ~0.707 -> ~180 (screen-linear, no gamma).
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.0, 0.0, 0.0];
        st.lights.dir[0] = DirLight {
            dir: [0.0, 0.0, 1.0],
            col: [1.0, 1.0, 1.0],
        };
        let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
        let c = light_vertex(&st, [inv_sqrt2, 0.0, inv_sqrt2]);
        // 0.707 * 255 ≈ 180.
        assert!((c[0] as i32 - 180).abs() <= 1, "got {}", c[0]);
    }

    #[test]
    fn light_vertex_modelview_rotates_light_into_local_space() {
        // computeDirLight brings the light dir into local space via the
        // modelview. With a 90° rotation about Y, a light along +X ends up
        // along the axis a +Z-facing normal is lit by. Concretely: rotate the
        // world +X light so it aligns with the vertex normal's frame, giving
        // full N·L where an unrotated dot would give 0.
        let mut st = lit_state();
        st.lights.num_dir = 1;
        st.lights.ambient = [0.0, 0.0, 0.0];
        st.lights.dir[0] = DirLight {
            dir: [1.0, 0.0, 0.0], // light along world +X
            col: [1.0, 1.0, 1.0],
        };
        // modelview that rotates +X -> +Z under rotate_dir (row-major,
        // column-vector): out.z = m[2][0]*x. Set m[2][0]=1, m[0][0]=0.
        let mut mv = identity();
        mv[0][0] = 0.0;
        mv[2][0] = 1.0;
        mv[2][2] = 0.0;
        st.modelview = mv;
        // Normal along +Z now sees the rotated light head-on.
        let c = light_vertex(&st, [0.0, 0.0, 1.0]);
        assert_eq!(c, [255, 255, 255]);
        // Sanity: WITHOUT the rotation (identity), the +X light and +Z normal
        // are orthogonal -> no diffuse.
        st.modelview = identity();
        let c0 = light_vertex(&st, [0.0, 0.0, 1.0]);
        assert_eq!(c0, [0, 0, 0]);
    }

    // --- Near-plane culling (the "fan from a point" fix) ----------------

    fn vtx_w(w: f32) -> Vertex {
        Vertex {
            w,
            ..Default::default()
        }
    }

    #[test]
    fn behind_near_plane_flags_nonpositive_w() {
        assert!(behind_near_plane(&vtx_w(-1.0)), "w<0 is behind camera");
        assert!(
            behind_near_plane(&vtx_w(0.0)),
            "w==0 is on the camera plane"
        );
        assert!(!behind_near_plane(&vtx_w(1.0)), "w>0 is in front");
    }

    #[test]
    fn resolve_tri_drops_triangle_with_a_behind_camera_vertex() {
        // Fail-against-bug: a triangle with one vertex at w<=0 is the "fan
        // from a point" artifact (projecting it flings it across the screen).
        // resolve_tri must DROP it, not emit a giant wrong-side polygon.
        let mut cache = [Vertex::default(); 64];
        cache[0] = vtx_w(1.0);
        cache[1] = vtx_w(1.0);
        cache[2] = vtx_w(-0.5); // behind the near plane
        assert!(
            resolve_tri(
                &cache,
                [0, 1, 2],
                CullMode::None,
                None,
                OtherMode::default(),
                CombinerState::default(),
                BlenderState::default(),
            )
            .is_none(),
            "triangle touching a behind-camera vertex must be dropped"
        );
        // All three in front -> kept.
        cache[2] = vtx_w(2.0);
        assert!(resolve_tri(
            &cache,
            [0, 1, 2],
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_some());
    }

    // --- Texture sampling (priority 4) ----------------------------------

    /// Build a 2×2 RGBA8888 texture: TL=red, TR=green, BL=blue, BR=white.
    fn checker_2x2(clamp: bool) -> Texture {
        let texels = vec![
            255, 0, 0, 255, // (0,0) red
            0, 255, 0, 255, // (1,0) green
            0, 0, 255, 255, // (0,1) blue
            255, 255, 255, 255, // (1,1) white
        ];
        Texture {
            format: 0,
            size: 2,
            width: 2,
            height: 2,
            texels: std::rc::Rc::new(texels),
            clamp_s: clamp,
            clamp_t: clamp,
            mirror_s: false,
            mirror_t: false,
            mask_s: if clamp { 0 } else { 1 },
            mask_t: if clamp { 0 } else { 1 },
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        }
    }

    #[test]
    fn yuyv_pairs_decode_to_shared_chroma_and_distinct_luma() {
        let mut rdram = vec![0u8; 0x200];
        for (index, value) in [16, 128, 235, 128].into_iter().enumerate() {
            wr_u8(&mut rdram, 0x100 + index, value);
        }
        let mut tex = TexState {
            timg_addr: 0x100,
            timg_width: 2,
            ..TexState::default()
        };
        tex.tiles[0].fmt = G_IM_FMT_YUV;
        tex.tiles[0].siz = G_IM_SIZ_16B;
        tex.tiles[0].lrs = 4;
        let texture = decode_current_texture(&rdram, &tex, &[0; 16], 0, TextureLoad::Block);
        assert_eq!(&texture.texels[..4], &[16, 128, 128, 255]);
        assert_eq!(&texture.texels[4..8], &[235, 128, 128, 255]);
    }

    #[test]
    #[should_panic(expected = "texture tile 0 uses unsupported format 1 size 1")]
    fn direct_texture_oracle_traps_instead_of_falling_back_to_flat_shading() {
        let mut tex = TexState::default();
        tex.tiles[0].fmt = G_IM_FMT_YUV;
        tex.tiles[0].siz = G_IM_SIZ_8B;
        let _ = decode_current_texture(&[0; 16], &tex, &[0; 16], 0, TextureLoad::Block);
    }

    #[test]
    fn texture_conversion_modes_execute_point_filter_and_filter_convert() {
        let convert = ConvertState::default();
        assert_eq!(
            convert.convert_texel([100, 128, 128, 255]),
            [100, 100, 100, 255]
        );
        assert_eq!(
            convert.convert_texel([100, 255, 255, 255]),
            [255, 0, 255, 255]
        );

        let texture = Texture {
            format: G_IM_FMT_YUV,
            size: G_IM_SIZ_16B,
            width: 2,
            height: 1,
            texels: std::rc::Rc::new(vec![20, 128, 128, 255, 220, 128, 128, 255]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };
        let base = OtherMode::default().raw_high() & !(7 << 9);
        let conv = OtherMode::from_raw(base, 0, 0);
        let filtconv = OtherMode::from_raw(base | (5 << 9) | (3 << 12), 0, 0);
        let filt = OtherMode::from_raw(base | (6 << 9) | (3 << 12), 0, 0);
        assert_eq!(texture.sample_rdp(0.75, 0.0, conv, convert)[0], 20);
        assert_eq!(texture.sample_rdp(0.5, 0.0, filtconv, convert)[0], 120);
        assert_eq!(texture.sample_rdp(0.5, 0.0, filt, convert)[0], 120);
    }

    #[test]
    fn lod_selection_matches_public_mip_detail_and_sharpen_tables() {
        let snapshot = TextureLodSnapshot {
            tiles: std::array::from_fn(|_| None),
            primitive_tile: 2,
            max_level: 3,
        };
        let base = OtherMode::default().raw_high() & !((1 << 16) | (3 << 17));
        let mode = |detail: u32| OtherMode::from_raw(base | (1 << 16) | (detail << 17), 0, 0);
        let derivatives = |lod: f32| TextureDerivatives {
            dsdx: lod,
            ..TextureDerivatives::default()
        };

        assert_eq!(
            Texture::lod_selection(&snapshot, derivatives(7.5), mode(0), 0),
            TextureLodSelection {
                tile0: 4,
                tile1: 5,
                fraction: 0.875,
            }
        );
        assert_eq!(
            Texture::lod_selection(&snapshot, derivatives(0.25), mode(0), 0),
            TextureLodSelection {
                tile0: 2,
                tile1: 2,
                fraction: 0.25,
            }
        );
        let detail = Texture::lod_selection(&snapshot, derivatives(0.25), mode(2), 128);
        assert_eq!((detail.tile0, detail.tile1), (2, 3));
        assert!((detail.fraction - 128.0 / 255.0).abs() < f32::EPSILON);
        assert_eq!(
            Texture::lod_selection(&snapshot, derivatives(0.5), mode(1), 0),
            TextureLodSelection {
                tile0: 2,
                tile1: 3,
                fraction: -0.5,
            }
        );
        assert_eq!(
            Texture::lod_selection(&snapshot, derivatives(2.5), mode(2), 0),
            TextureLodSelection {
                tile0: 4,
                tile1: 5,
                fraction: 0.25,
            }
        );
    }

    #[test]
    #[should_panic(expected = "RDP combiner selected TEXEL1 without a decoded tile+1 image")]
    fn missing_texel1_never_aliases_texel0() {
        checker_2x2(true).sample_rdp_pair(
            None,
            TextureSampleRequest {
                s: 0.0,
                t: 0.0,
                derivatives: TextureDerivatives::default(),
                other_mode: OtherMode::default(),
                convert: ConvertState::default(),
                min_level: 0,
                require_texel1: true,
            },
        );
    }

    #[test]
    #[should_panic(
        expected = "RDP LOD selected tile 1 without a decoded G_LOADBLOCK/G_LOADTILE image"
    )]
    fn missing_lod_selected_tile_traps_by_index() {
        let tile0 = checker_2x2(true);
        let mut tiles = std::array::from_fn(|_| None);
        tiles[0] = Some(tile0.clone());
        let texture = tile0.with_lod_snapshot(tiles, 0, 2);
        let high = (OtherMode::default().raw_high() & !(1 << 16)) | (1 << 16);
        texture.sample_rdp_pair(
            None,
            TextureSampleRequest {
                s: 0.0,
                t: 0.0,
                derivatives: TextureDerivatives {
                    dsdx: 2.5,
                    ..TextureDerivatives::default()
                },
                other_mode: OtherMode::from_raw(high, 0, 0),
                convert: ConvertState::default(),
                min_level: 0,
                require_texel1: false,
            },
        );
    }

    #[test]
    fn texture_and_primitive_commands_retain_lod_limits() {
        let mut rdram = vec![0u8; 0x1020];
        wr_cmd(
            &mut rdram,
            0x1000,
            ((G_TEXTURE as u32) << 24) | (5 << 11) | (3 << 8) | 2,
            0xffff_ffff,
        );
        wr_cmd(
            &mut rdram,
            0x1008,
            ((G_SETPRIMCOLOR as u32) << 24) | (0x80 << 8) | 0x40,
            0x0102_0304,
        );
        wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

        let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
        assert!(state.tex.tex_enabled);
        assert_eq!((state.tex.tex_tile, state.tex.tex_max_level), (3, 5));
        assert_eq!(state.combiner.min_lod_level, 0x80);
        assert_eq!(state.combiner.prim_lod_fraction, 0x40);
    }

    fn indexed_texture(width: u32) -> Texture {
        let texels = (0..width)
            .flat_map(|index| [index as u8, 0, 0, 255])
            .collect();
        Texture {
            format: G_IM_FMT_RGBA,
            size: G_IM_SIZ_32B,
            width,
            height: 1,
            texels: std::rc::Rc::new(texels),
            clamp_s: false,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        }
    }

    #[test]
    fn texture_samples_the_right_texel() {
        let tex = checker_2x2(true);
        // Each integer texel coordinate lands on its own texel (nearest).
        assert_eq!(tex.sample(0.0, 0.0), [255, 0, 0, 255]); // TL red
        assert_eq!(tex.sample(1.0, 0.0), [0, 255, 0, 255]); // TR green
        assert_eq!(tex.sample(0.0, 1.0), [0, 0, 255, 255]); // BL blue
        assert_eq!(tex.sample(1.0, 1.0), [255, 255, 255, 255]); // BR white

        // Fractional coords floor to the containing texel.
        assert_eq!(tex.sample(0.9, 0.1), [255, 0, 0, 255]); // floor -> (0,0) red
    }

    #[test]
    fn texture_sample_floor_addressing() {
        let tex = checker_2x2(true);
        // (1.5, 0.9) floors to (1, 0) = green.
        assert_eq!(tex.sample(1.5, 0.9), [0, 255, 0, 255]);
        // (0.2, 1.7) floors to (0, 1) = blue.
        assert_eq!(tex.sample(0.2, 1.7), [0, 0, 255, 255]);
    }

    #[test]
    fn texture_filter_matches_public_point_average_and_triangular_rules() {
        let texture = Texture {
            format: 0,
            size: 2,
            width: 2,
            height: 2,
            texels: std::rc::Rc::new(vec![
                0, 0, 0, 0, // c00
                100, 100, 100, 100, // c10
                200, 200, 200, 200, // c01
                255, 255, 255, 255, // c11
            ]),
            clamp_s: true,
            clamp_t: true,
            mirror_s: false,
            mirror_t: false,
            mask_s: 0,
            mask_t: 0,
            shift_s: 0,
            shift_t: 0,
            origin_s: 0.0,
            origin_t: 0.0,
            tmem: None,
            lod: None,
        };

        assert_eq!(
            texture.sample_filtered(0.75, 0.75, TextureFilter::Point),
            [0; 4]
        );
        assert_eq!(
            texture.sample_filtered(0.5, 0.5, TextureFilter::Average),
            [139; 4]
        );
        assert_eq!(
            texture.sample_filtered(0.25, 0.25, TextureFilter::Bilinear),
            [75; 4],
            "upper triangle must interpolate c00/c10/c01"
        );
        assert_eq!(
            texture.sample_filtered(0.75, 0.75, TextureFilter::Bilinear),
            [203; 4],
            "lower triangle must interpolate c11/c01/c10"
        );
    }

    #[test]
    fn texture_s10_5_coordinate_sweep_covers_every_grid_value_and_tile_shift() {
        for raw in i16::MIN..=i16::MAX {
            let coordinate = TextureCoordinateS10_5::from_texels_bounded(f32::from(raw) / 32.0);
            assert_eq!(coordinate.0, raw);
            assert_eq!(coordinate.shifted(0).texel(), i64::from(raw).div_euclid(32));
            assert_eq!(
                coordinate.shifted(0).fraction(),
                i64::from(raw).rem_euclid(32)
            );
            for encoded in 0..=15 {
                let expected = match encoded {
                    0 => i64::from(raw),
                    1..=10 => i64::from(raw) >> encoded,
                    11..=15 => i64::from(raw) * (1_i64 << (16 - encoded)),
                    _ => unreachable!(),
                };
                assert_eq!(
                    coordinate.shifted(encoded).0,
                    expected,
                    "raw={raw} shift={encoded}"
                );
            }
        }

        assert!(std::panic::catch_unwind(|| {
            TextureCoordinateS10_5::from_texels_bounded(f32::NAN)
        })
        .is_err());
        for outside in [-1024.0 - 1.0 / 32.0, 1024.0] {
            assert!(
                std::panic::catch_unwind(|| {
                    TextureCoordinateS10_5::from_texels_bounded(outside)
                })
                .is_err(),
                "outside={outside}"
            );
        }
        assert_eq!(
            TextureCoordinateS10_5::from_texels_bounded(-1024.0).0,
            i16::MIN
        );
        assert_eq!(
            TextureCoordinateS10_5::from_texels_bounded(1024.0 - 1.0 / 32.0).0,
            i16::MAX
        );
    }

    #[test]
    fn texture_fixed_s10_5_filter_sweeps_both_triangle_halves_without_float_drift() {
        let mut lower_half = 0usize;
        let mut upper_half = 0usize;
        for seed in 0..=255u16 {
            let values = [
                seed as u8,
                seed.wrapping_mul(73).wrapping_add(19) as u8,
                seed.wrapping_mul(151).wrapping_add(41) as u8,
                seed.wrapping_mul(211).wrapping_add(97) as u8,
            ];
            let samples = values.map(|value| [value; 4]);
            for sf in 0..32i64 {
                for tf in 0..32i64 {
                    let [c00, c10, c01, c11] = values.map(f32::from);
                    let sf_float = sf as f32 / 32.0;
                    let tf_float = tf as f32 / 32.0;
                    let expected = if sf + tf <= 32 {
                        lower_half += 1;
                        c00 + sf_float * (c10 - c00) + tf_float * (c01 - c00)
                    } else {
                        upper_half += 1;
                        c11 + (1.0 - sf_float) * (c01 - c11) + (1.0 - tf_float) * (c10 - c11)
                    }
                    .round()
                    .clamp(0.0, 255.0) as u8;
                    assert_eq!(
                        filter_three_nearest_s10_5(samples, sf, tf),
                        [expected; 4],
                        "seed={seed} sf={sf}/32 tf={tf}/32"
                    );
                }
            }
        }
        assert_eq!((lower_half, upper_half), (143_104, 119_040));
    }

    #[test]
    fn texture_fixed_s10_5_negative_half_texel_observes_wrap_mirror_and_clamp_boundaries() {
        let mut texture = indexed_texture(4);
        texture.mask_s = 2;
        texture.clamp_s = false;
        texture.texels = std::rc::Rc::new(
            [0u8, 64, 128, 255]
                .into_iter()
                .flat_map(|value| [value; 4])
                .collect(),
        );

        assert_eq!(texture.sample(-1.0 / 32.0, 0.0), [255; 4]);
        assert_eq!(
            texture.sample_filtered(1.0 / 64.0, 0.0, TextureFilter::Bilinear),
            [0; 4],
            "bounded host conversion floors a positive sub-grid fraction to S10.5 zero"
        );
        assert_eq!(
            texture.sample_filtered(-1.0 / 64.0, 0.0, TextureFilter::Bilinear),
            [8; 4],
            "bounded host conversion floors a negative sub-grid fraction to -1/32"
        );
        assert_eq!(
            texture.sample_filtered(-0.5, 0.0, TextureFilter::Bilinear),
            [128; 4],
            "wrapped -1/2 selects texels 3 and 0 at equal S10.5 weights"
        );
        texture.mirror_s = true;
        assert_eq!(texture.sample(-1.0 / 32.0, 0.0), [0; 4]);
        texture.clamp_s = true;
        assert_eq!(
            texture.sample_filtered(-0.5, 0.0, TextureFilter::Bilinear),
            [0; 4]
        );

        for raw in -96..=96i16 {
            let coordinate = TextureCoordinateS10_5(raw);
            for shift in 0..=15 {
                let shifted = coordinate.shifted(shift).texel();
                for mask in 0..=15 {
                    for clamp in [false, true] {
                        for mirror in [false, true] {
                            let clamped = if mask == 0 || clamp {
                                shifted.clamp(0, 36)
                            } else {
                                shifted
                            };
                            let expected = if mask == 0 {
                                clamped as u32
                            } else {
                                let low_mask = (1_i64 << mask) - 1;
                                if mirror && clamped & (1_i64 << mask) != 0 {
                                    ((!clamped) & low_mask) as u32
                                } else {
                                    (clamped & low_mask) as u32
                                }
                            };
                            assert_eq!(
                                texture_axis_address(
                                    shifted,
                                    37,
                                    clamp,
                                    mirror,
                                    mask,
                                    TextureAddressMode::Programmed,
                                ),
                                expected,
                                "raw={raw} shift={shift} mask={mask} clamp={clamp} mirror={mirror}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn texture_shift_precedes_fractional_tile_origin_subtraction() {
        let mut texture = indexed_texture(8);
        texture.clamp_s = true;
        texture.origin_s = 0.5;

        texture.shift_s = 1;
        assert_eq!(
            texture.sample(6.5, 0.0)[0],
            2,
            "right shift first gives 6.5/2-0.5=2.75; subtract-first would select 3"
        );

        texture.shift_s = 15;
        assert_eq!(
            texture.sample(1.25, 0.0)[0],
            2,
            "left shift first gives 1.25*2-0.5=2; subtract-first would select 1"
        );
    }

    #[test]
    fn texture_clamp_vs_wrap_addressing() {
        let clamp = checker_2x2(true);
        // Out-of-range clamps to the edge texel.
        assert_eq!(clamp.sample(5.0, 0.0), [0, 255, 0, 255]); // clamp to x=1 green
        assert_eq!(clamp.sample(-3.0, 1.0), [0, 0, 255, 255]); // clamp to x=0 blue

        let wrap = checker_2x2(false);
        // Wrap repeats: s=2 -> texel 0, s=3 -> texel 1, s=-1 -> texel 1.
        assert_eq!(wrap.sample(2.0, 0.0), [255, 0, 0, 255]); // (0,0) red
        assert_eq!(wrap.sample(3.0, 0.0), [0, 255, 0, 255]); // (1,0) green
        assert_eq!(wrap.sample(-1.0, 0.0), [0, 255, 0, 255]); // wraps to (1,0)
    }

    #[test]
    fn copy_clamp_axis_sweep_matches_public_bypass_then_wrap_mirror_equation() {
        const DIMENSION: u32 = 37;
        for mode in [TextureAddressMode::Programmed, TextureAddressMode::Copy] {
            for mask in 0..=15u8 {
                for clamp in [false, true] {
                    for mirror in [false, true] {
                        for input in -1024..=1023i64 {
                            let clamps =
                                mode == TextureAddressMode::Programmed && (mask == 0 || clamp);
                            let coordinate = if clamps {
                                input.clamp(0, i64::from(DIMENSION) - 1)
                            } else {
                                input
                            };
                            let expected = if mask == 0 {
                                coordinate as u32
                            } else {
                                let low_mask = (1_i64 << mask) - 1;
                                if mirror && coordinate & (1_i64 << mask) != 0 {
                                    ((!coordinate) & low_mask) as u32
                                } else {
                                    (coordinate & low_mask) as u32
                                }
                            };
                            assert_eq!(
                                texture_axis_address(
                                    input,
                                    DIMENSION,
                                    clamp,
                                    mirror,
                                    mask,
                                    mode,
                                ),
                                expected,
                                "mode={mode:?} coordinate={input} mask={mask} clamp={clamp} mirror={mirror}"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(
            texture_axis_address(99, 0, true, true, 15, TextureAddressMode::Copy),
            0
        );
    }

    #[test]
    fn texture_mask_mirror_and_clamp_follow_public_coordinate_sequences() {
        let mut texture = indexed_texture(4);
        texture.mask_s = 2;
        assert_eq!(
            (0..8)
                .map(|s| texture.sample(s as f32, 0.0)[0])
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 0, 1, 2, 3]
        );

        texture.mirror_s = true;
        assert_eq!(
            (0..8)
                .map(|s| texture.sample(s as f32, 0.0)[0])
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 3, 2, 1, 0],
            "Programming Manual Chapter 13 mask=2 mirror sequence"
        );

        let mut clamp_after_one_mirror = indexed_texture(12);
        clamp_after_one_mirror.mask_s = 2;
        clamp_after_one_mirror.mirror_s = true;
        clamp_after_one_mirror.clamp_s = true;
        assert_eq!(
            (8..16)
                .map(|s| clamp_after_one_mirror.sample(s as f32, 0.0)[0])
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 3, 3, 3, 3],
            "clamp must freeze the input at SH before mirror/mask addressing"
        );
    }

    #[test]
    fn texture_shift_decodes_public_right_and_left_ranges() {
        let mut texture = indexed_texture(64);
        texture.clamp_s = true;
        texture.shift_s = 1;
        assert_eq!(texture.sample(6.0, 0.0)[0], 3);
        texture.shift_s = 11;
        assert_eq!(texture.sample(1.0, 0.0)[0], 32);
        texture.shift_s = 15;
        assert_eq!(texture.sample(3.0, 0.0)[0], 6);
    }

    #[test]
    fn set_tile_retains_all_public_address_fields_and_existing_extent() {
        let mut tile = Tile {
            uls: 4,
            ult: 8,
            lrs: 12,
            lrt: 16,
            ..Default::default()
        };
        let w0 = (G_IM_FMT_CI as u32) << 21 | (G_IM_SIZ_8B as u32) << 19 | 0x155 << 9 | 0x12a;
        let w1 = 7 << 24 | 9 << 20 | 3 << 18 | 5 << 14 | 12 << 10 | 1 << 8 | 4 << 4 | 15;
        apply_set_tile(&mut tile, w0, w1);

        assert_eq!(tile.fmt, G_IM_FMT_CI);
        assert_eq!(tile.siz, G_IM_SIZ_8B);
        assert_eq!(tile.line, 0x155);
        assert_eq!(tile.tmem, 0x12a);
        assert_eq!(tile.palette, 9);
        assert!(tile.clamp_t && tile.mirror_t);
        assert!(!tile.clamp_s && tile.mirror_s);
        assert_eq!((tile.mask_s, tile.mask_t), (4, 5));
        assert_eq!((tile.shift_s, tile.shift_t), (15, 12));
        assert_eq!((tile.uls, tile.ult, tile.lrs, tile.lrt), (4, 8, 12, 16));
    }

    #[test]
    fn rgba5551_expands_high_bits() {
        // Pure red (R5=0x1F) -> R8=0xFF; alpha bit set -> 0xFF.
        assert_eq!(rgba5551_to_rgba8888(0xF801), [255, 0, 0, 255]);
        // Pure green (G5=0x1F at bits 6..10).
        assert_eq!(rgba5551_to_rgba8888(0x07C1), [0, 255, 0, 255]);
        // Black, alpha 0.
        assert_eq!(rgba5551_to_rgba8888(0x0000), [0, 0, 0, 0]);
    }

    #[test]
    fn color_image_layout_classifies_exactly_the_public_memory_interfaces() {
        let image = |format, size| ColorImage {
            format,
            size,
            width: 1,
            address: 0,
        };
        assert_eq!(
            image(ColorImage::CI_FORMAT, ColorImage::BITS_8).layout(),
            Some(ColorImageLayout::Index8)
        );
        assert_eq!(
            image(4, ColorImage::BITS_8).layout(),
            Some(ColorImageLayout::Index8),
            "the public 8-bit memory interface is selected by size"
        );
        assert_eq!(
            image(ColorImage::RGBA_FORMAT, ColorImage::BITS_16).layout(),
            Some(ColorImageLayout::Rgba16)
        );
        assert_eq!(
            image(ColorImage::RGBA_FORMAT, ColorImage::BITS_32).layout(),
            Some(ColorImageLayout::Rgba32)
        );
        for (format, size) in [(1, 2), (2, 2), (3, 3), (0, 0)] {
            assert_eq!(
                image(format, size).layout(),
                None,
                "format={format} size={size}"
            );
        }
    }

    #[test]
    fn color_image_transition_matrix_admits_every_public_pair() {
        let image = |layout| ColorImage {
            format: match layout {
                ColorImageLayout::Index8 => ColorImage::CI_FORMAT,
                ColorImageLayout::Rgba16 | ColorImageLayout::Rgba32 => ColorImage::RGBA_FORMAT,
            },
            size: match layout {
                ColorImageLayout::Index8 => ColorImage::BITS_8,
                ColorImageLayout::Rgba16 => ColorImage::BITS_16,
                ColorImageLayout::Rgba32 => ColorImage::BITS_32,
            },
            width: 4,
            address: 0,
        };

        for from in ColorImageLayout::ALL {
            for to in ColorImageLayout::ALL {
                assert_eq!(
                    image(from).transition_to(image(to)),
                    ColorImageLayoutTransition { from, to }
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "unsupported destination color-image layout")]
    fn color_image_transition_traps_an_unsupported_destination() {
        ColorImage {
            format: ColorImage::RGBA_FORMAT,
            size: ColorImage::BITS_16,
            width: 1,
            address: 0,
        }
        .transition_to(ColorImage {
            format: ColorImage::CI_FORMAT,
            size: ColorImage::BITS_16,
            width: 1,
            address: 0,
        });
    }

    #[test]
    fn load_tlut_count_uses_all_ten_wire_bits() {
        // Public gbi.h encodes `count - 1` directly, without quarter-texel
        // scaling. Discarding the low two bits turns the normal 256-entry CI8
        // palette into 64 entries.
        assert_eq!(load_tlut_count(255 << 14), 256);
        assert_eq!(load_tlut_count(15 << 14), 16);
    }

    #[test]
    fn tmem_storage_matches_public_odd_row_and_rgba32_bank_layouts() {
        let mut storage = Tmem::default();
        let rgba16 = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_16B,
            line: 1,
            ..Default::default()
        };
        storage.write_texel(rgba16, 0, 0, false, G_IM_SIZ_16B, 0x1122);
        storage.write_texel(rgba16, 0, 1, true, G_IM_SIZ_16B, 0x3344);
        assert_eq!(&storage.bytes[0..2], &[0x11, 0x22]);
        // Row 1 logical byte 8 is exchanged into the upper 32-bit long.
        assert_eq!(&storage.bytes[12..14], &[0x33, 0x44]);
        assert_eq!(storage.read_texel(rgba16, 0, 0, G_IM_SIZ_16B), 0x1122);
        assert_eq!(storage.read_texel(rgba16, 0, 1, G_IM_SIZ_16B), 0x3344);

        let mut storage = Tmem::default();
        let rgba32 = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_32B,
            line: 1,
            ..Default::default()
        };
        storage.write_texel(rgba32, 0, 0, false, G_IM_SIZ_32B, 0x1122_3344);
        storage.write_texel(rgba32, 0, 1, true, G_IM_SIZ_32B, 0x5566_7788);
        assert_eq!(&storage.bytes[0..2], &[0x11, 0x22]);
        assert_eq!(
            &storage.bytes[TMEM_HALF_BYTES..TMEM_HALF_BYTES + 2],
            &[0x33, 0x44]
        );
        assert_eq!(&storage.bytes[12..14], &[0x55, 0x66]);
        assert_eq!(
            &storage.bytes[TMEM_HALF_BYTES + 12..TMEM_HALF_BYTES + 14],
            &[0x77, 0x88]
        );
        assert_eq!(storage.read_texel(rgba32, 0, 0, G_IM_SIZ_32B), 0x1122_3344);
        assert_eq!(storage.read_texel(rgba32, 0, 1, G_IM_SIZ_32B), 0x5566_7788);

        let mut storage = Tmem::default();
        let i4 = Tile {
            fmt: G_IM_FMT_I,
            siz: G_IM_SIZ_4B,
            line: 1,
            ..Default::default()
        };
        for x in 0..16 {
            storage.write_texel(i4, x, 1, true, G_IM_SIZ_4B, x as u32);
        }
        // On an odd row, texels 0..7 occupy the second 32-bit long and
        // texels 8..15 occupy the first, as in Manual Figure 13.8.3.
        assert_eq!(&storage.bytes[8..12], &[0x89, 0xab, 0xcd, 0xef]);
        assert_eq!(&storage.bytes[12..16], &[0x01, 0x23, 0x45, 0x67]);
    }

    #[test]
    fn yuv_tmem_splits_chroma_low_and_luma_high() {
        let mut storage = Tmem::default();
        let tile = Tile {
            fmt: G_IM_FMT_YUV,
            siz: G_IM_SIZ_16B,
            line: 1,
            ..Default::default()
        };
        storage.write_yuv_pair(tile, 0, 0, false, [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(&storage.bytes[0..2], &[0x20, 0x40]);
        assert_eq!(
            &storage.bytes[TMEM_HALF_BYTES..TMEM_HALF_BYTES + 2],
            &[0x10, 0x30]
        );
        let texture = TmemTexture {
            storage: std::rc::Rc::new(storage),
            tile,
            texture_lut: 0,
        };
        assert_eq!(texture.sample(0, 0), [0x10, 0x20, 0x40, 255]);
        assert_eq!(texture.sample(1, 0), [0x30, 0x20, 0x40, 255]);
    }

    #[test]
    fn load_and_render_tiles_share_snapshotted_tmem_beyond_extent() {
        let base = 0x100usize;
        let source = [0xf801u16, 0x07c1, 0x003f, 0xffff];
        let mut rdram = vec![0; base + 16];
        for (index, value) in source.into_iter().enumerate() {
            let [hi, lo] = value.to_be_bytes();
            wr_u8(&mut rdram, base + index * 2, hi);
            wr_u8(&mut rdram, base + index * 2 + 1, lo);
        }
        let mut tex = TexState {
            timg_addr: base as u32,
            timg_fmt: G_IM_FMT_RGBA,
            timg_siz: G_IM_SIZ_16B,
            timg_width: 4,
            ..Default::default()
        };
        tex.tiles[7] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_16B,
            ..Default::default()
        };
        load_block_into_tmem(
            &rdram,
            &mut tex,
            &[0; 16],
            7,
            u32::from(G_LOADBLOCK) << 24,
            (7 << 24) | (3 << 12),
        );
        // A separate render tile reinterprets the same TMEM word. Its active
        // clamp extent is only two texels, but mask=2 deliberately addresses
        // all four loaded texels.
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_16B,
            line: 1,
            mask_s: 2,
            lrs: 4,
            ..Default::default()
        };
        let texture = bind_texture_set(&tex, 0, 0, 0).expect("render tile must bind TMEM");
        assert_eq!(texture.sample(0.0, 0.0), [255, 0, 0, 255]);
        assert_eq!(texture.sample(3.0, 0.0), [255, 255, 255, 255]);

        std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
            tex.tiles[0],
            0,
            0,
            false,
            G_IM_SIZ_16B,
            0x003f,
        );
        assert_eq!(
            texture.sample(0.0, 0.0),
            [255, 0, 0, 255],
            "a later TMEM load must not mutate an emitted primitive"
        );
        let reloaded = bind_texture_set(&tex, 0, 0, 0).expect("reloaded tile must bind");
        assert_eq!(reloaded.sample(0.0, 0.0), [0, 0, 255, 255]);
    }

    #[test]
    fn wrapped_tile_accepts_an_origin_above_its_unused_clamp_bound() {
        let mut tex = TexState::default();
        tex.tiles[1] = Tile {
            fmt: G_IM_FMT_I,
            siz: G_IM_SIZ_8B,
            line: 1,
            mask_t: 5,
            ult: 0x0ffd,
            lrt: 0,
            ..Default::default()
        };
        std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
            tex.tiles[1],
            0,
            0,
            true,
            G_IM_SIZ_8B,
            0x7f,
        );

        let texture = texture_for_tile(&tex, 1, 0, &tex.tmem)
            .expect("wrap mask, not unused clamp bounds, defines tile validity");
        assert_eq!(texture.height, 32);
        assert_eq!(texture.sample(0.0, 0.0), [0x7f; 4]);
    }

    #[test]
    fn texel1_gap_reversed_clamp_extent_is_invalid_without_eager_unsigned_subtraction() {
        let mut tex = TexState::default();
        tex.tiles[1] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_16B,
            line: 1,
            clamp_s: true,
            uls: 4,
            lrs: 0,
            ..Default::default()
        };
        std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
            tex.tiles[1],
            0,
            0,
            false,
            G_IM_SIZ_16B,
            0xf801,
        );

        assert!(
            texture_for_tile(&tex, 1, 0, &tex.tmem).is_none(),
            "a reversed clamped extent is not a bindable tile"
        );
    }

    #[test]
    fn ci4_samples_quadricated_tlut_at_palette_bank_address() {
        let mut storage = Tmem::default();
        let tile = Tile {
            fmt: G_IM_FMT_CI,
            siz: G_IM_SIZ_4B,
            palette: 2,
            line: 1,
            ..Default::default()
        };
        storage.write_texel(tile, 0, 0, false, G_IM_SIZ_4B, 1);
        storage.write_tlut(256, 0x21, 0xf801);
        let texture = TmemTexture {
            storage: std::rc::Rc::new(storage),
            tile,
            texture_lut: 2,
        };
        assert_eq!(texture.sample(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn load_tile_uses_settimg_stride_and_tile_coordinate_origin() {
        // A synthetic 4x2 CI8 source. Load the rightmost two texels of row 1
        // as a 2x1 tile whose render coordinates begin at (2, 1).
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + 12];
        for (i, index) in (0u8..8).enumerate() {
            wr_u8(&mut rdram, base + i, index);
        }
        let mut tlut = vec![[0, 0, 0, 255]; 8];
        tlut[6] = [60, 61, 62, 255];
        tlut[7] = [70, 71, 72, 255];
        let mut tex = TexState {
            timg_addr: base as u32,
            timg_width: 4,
            tlut,
            ..Default::default()
        };
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_CI,
            siz: G_IM_SIZ_8B,
            uls: 2 * 4,
            ult: 4,
            lrs: 3 * 4,
            lrt: 4,
            clamp_s: true,
            clamp_t: true,
            ..Default::default()
        };

        let decoded = decode_current_texture(
            &rdram,
            &tex,
            &[0; 16],
            0,
            TextureLoad::Tile {
                source_x: 2,
                source_y: 1,
            },
        );

        assert_eq!(
            decoded.texels.as_slice(),
            &[60, 61, 62, 255, 70, 71, 72, 255]
        );
        assert_eq!(decoded.sample(2.0, 1.0), [60, 61, 62, 255]);
        assert_eq!(decoded.sample(3.0, 1.0), [70, 71, 72, 255]);
    }

    #[test]
    fn load_tile_preserves_equal_fractional_bounds_as_subtexel_origin() {
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + 8];
        for (index, value) in [10, 20, 30, 40].into_iter().enumerate() {
            wr_u8(&mut rdram, base + index, value);
        }
        let mut tex = TexState {
            timg_addr: base as u32,
            timg_fmt: G_IM_FMT_I,
            timg_siz: G_IM_SIZ_8B,
            timg_width: 4,
            ..Default::default()
        };
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_I,
            siz: G_IM_SIZ_8B,
            line: 1,
            clamp_s: true,
            clamp_t: true,
            ..Default::default()
        };

        // Load source texels 1..=2 with a quarter-texel S origin and a
        // half-texel T origin. Table 7 retains the fractions in tile state;
        // equal low/high fractions select the same integer source span.
        load_tile_into_tmem(
            &rdram,
            &mut tex,
            &[0; 16],
            0,
            (u32::from(G_LOADTILE) << 24) | (5 << 12) | 2,
            (9 << 12) | 2,
        );
        let texture = bind_texture_set(&tex, 0, 0, 0).expect("fractional tile must bind");
        assert_eq!(texture.origin_s, 1.25);
        assert_eq!(texture.origin_t, 0.5);
        assert_eq!(texture.sample(1.25, 0.5), [20, 20, 20, 20]);
        assert_eq!(texture.sample(2.25, 0.5), [30, 30, 30, 30]);
    }

    #[test]
    fn load_tile_unequal_fractional_edges_select_integer_span_and_retain_bounds() {
        let mut tex = TexState {
            timg_addr: 0,
            timg_fmt: G_IM_FMT_I,
            timg_siz: G_IM_SIZ_8B,
            timg_width: 1,
            ..Default::default()
        };
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_I,
            siz: G_IM_SIZ_8B,
            line: 1,
            ..Default::default()
        };
        load_tile_into_tmem(
            &[0x7f; 8],
            &mut tex,
            &[0; 16],
            0,
            u32::from(G_LOADTILE) << 24,
            2 << 12,
        );
        assert_eq!((tex.tiles[0].uls, tex.tiles[0].lrs), (0, 2));
        let texture = bind_texture_set(&tex, 0, 0, 0).expect("fractional tile must bind");
        assert_eq!(texture.sample(0.0, 0.0), [0x7f; 4]);
    }

    #[test]
    fn load_tile_uses_texture_image_size_for_rgba32_split_storage() {
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + 12];
        for (index, value) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
            .into_iter()
            .enumerate()
        {
            wr_u8(&mut rdram, base + index, value);
        }
        let mut tex = TexState {
            timg_addr: base as u32,
            timg_fmt: G_IM_FMT_RGBA,
            timg_siz: G_IM_SIZ_32B,
            timg_width: 2,
            ..Default::default()
        };
        // The public Set Tile usage note requires a 16-bit load descriptor
        // for RGBA32 even though the source-sized transfer is split across
        // low/high TMEM halves.
        tex.tiles[7] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_16B,
            line: 1,
            ..Default::default()
        };
        load_tile_into_tmem(
            &rdram,
            &mut tex,
            &[0; 16],
            7,
            u32::from(G_LOADTILE) << 24,
            (7 << 24) | (4 << 12),
        );
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_32B,
            line: 1,
            lrs: 4,
            ..Default::default()
        };
        let texture = bind_texture_set(&tex, 0, 0, 0).expect("RGBA32 tile must bind");
        assert_eq!(texture.sample(0.0, 0.0), [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(texture.sample(1.0, 0.0), [0x50, 0x60, 0x70, 0x80]);
    }

    #[test]
    fn load_block_counts_source_sized_texels_with_mismatched_load_tile() {
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + 12];
        for (index, value) in [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
            .into_iter()
            .enumerate()
        {
            wr_u8(&mut rdram, base + index, value);
        }
        let mut tex = TexState {
            timg_addr: base as u32,
            timg_fmt: G_IM_FMT_RGBA,
            timg_siz: G_IM_SIZ_32B,
            timg_width: 2,
            ..Default::default()
        };
        tex.tiles[7] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_16B,
            ..Default::default()
        };
        load_block_into_tmem(
            &rdram,
            &mut tex,
            &[0; 16],
            7,
            u32::from(G_LOADBLOCK) << 24,
            (7 << 24) | (1 << 12),
        );
        tex.tiles[0] = Tile {
            fmt: G_IM_FMT_RGBA,
            siz: G_IM_SIZ_32B,
            line: 1,
            lrs: 4,
            ..Default::default()
        };
        let texture = bind_texture_set(&tex, 0, 0, 0).expect("RGBA32 block must bind");
        assert_eq!(texture.sample(0.0, 0.0), [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(texture.sample(1.0, 0.0), [0x55, 0x66, 0x77, 0x88]);
    }

    fn assert_texture_row(
        bytes: &[u8],
        width: u16,
        fmt: u8,
        siz: u8,
        palette: u8,
        tlut: Vec<[u8; 4]>,
        expected: &[u8],
    ) {
        let base = 0x100usize;
        let mut rdram = vec![0u8; base + bytes.len() + 4];
        for (i, &byte) in bytes.iter().enumerate() {
            wr_u8(&mut rdram, base + i, byte);
        }

        let mut tex = TexState {
            timg_addr: base as u32,
            tlut,
            ..Default::default()
        };
        tex.tiles[0] = Tile {
            fmt,
            siz,
            palette,
            lrs: (width - 1) * 4,
            ..Default::default()
        };
        assert_eq!(
            decode_current_texture(&rdram, &tex, &[0; 16], 0, TextureLoad::Block)
                .texels
                .as_slice(),
            expected
        );
    }

    #[test]
    fn decode_rgba16_covers_low_channels_and_alpha_edges() {
        // 0x0001 = opaque black; 0xffff = opaque white; 0x0842 has the
        // lowest nonzero R/G/B codes and clear alpha. This catches both a
        // dropped 1-bit alpha and incorrect 5-to-8 scaling at the low edge.
        assert_texture_row(
            &[0x00, 0x01, 0xff, 0xff, 0x08, 0x42],
            3,
            G_IM_FMT_RGBA,
            G_IM_SIZ_16B,
            0,
            Vec::new(),
            &[0, 0, 0, 255, 255, 255, 255, 255, 8, 8, 8, 0],
        );
    }

    #[test]
    fn decode_rgba8_uses_observed_hardware_i8_alias() {
        // Fail-against-bug: this pair previously fell through to None and
        // left the surface flat. RT64 records that hardware samples it as I8.
        assert_texture_row(
            &[0x24, 0xdb],
            2,
            G_IM_FMT_RGBA,
            G_IM_SIZ_8B,
            0,
            Vec::new(),
            &[0x24, 0x24, 0x24, 0x24, 0xdb, 0xdb, 0xdb, 0xdb],
        );
    }

    #[test]
    fn decode_rgba4_uses_observed_hardware_i4_alias() {
        // Fail-against-bug and live-OoT case: RGBA4 was one of the `_ =>
        // None` combinations, so every such tile remained flat-shaded.
        assert_texture_row(
            &[0x39],
            2,
            G_IM_FMT_RGBA,
            G_IM_SIZ_4B,
            0,
            Vec::new(),
            &[0x33, 0x33, 0x33, 0x33, 0x99, 0x99, 0x99, 0x99],
        );
    }

    #[test]
    fn decode_ia8_splits_four_bit_intensity_and_alpha() {
        assert_texture_row(
            &[0x1e, 0xf0],
            2,
            G_IM_FMT_IA,
            G_IM_SIZ_8B,
            0,
            Vec::new(),
            &[0x11, 0x11, 0x11, 0xee, 0xff, 0xff, 0xff, 0x00],
        );
    }

    #[test]
    fn decode_ia4_is_three_bit_intensity_plus_one_bit_alpha() {
        // Fail-against-bug: the old shared I4/IA4 arm expanded the whole
        // nibble into every channel. In particular 0x1 became translucent
        // dark gray and 0xe became opaque light gray. IA4 requires those to
        // be opaque black and transparent white respectively.
        assert_texture_row(
            &[0x1e, 0xa7],
            4,
            G_IM_FMT_IA,
            G_IM_SIZ_4B,
            0,
            Vec::new(),
            &[
                0, 0, 0, 255, // 0x1: I=0, A=1
                255, 255, 255, 0, // 0xe: I=7, A=0
                182, 182, 182, 0, // 0xa: I=5, A=0
                109, 109, 109, 255, // 0x7: I=3, A=1
            ],
        );
    }

    #[test]
    fn decode_i8_replicates_intensity_into_rgba() {
        assert_texture_row(
            &[0x00, 0x7f, 0xff],
            3,
            G_IM_FMT_I,
            G_IM_SIZ_8B,
            0,
            Vec::new(),
            &[0, 0, 0, 0, 0x7f, 0x7f, 0x7f, 0x7f, 0xff, 0xff, 0xff, 0xff],
        );
    }

    #[test]
    fn decode_ci8_uses_full_byte_as_rgba16_tlut_index() {
        let mut tlut = vec![[0, 0, 0, 0]; 256];
        tlut[0] = [1, 2, 3, 4];
        tlut[0x7f] = [5, 6, 7, 8];
        tlut[0xff] = [9, 10, 11, 12];
        assert_texture_row(
            &[0x00, 0x7f, 0xff],
            3,
            G_IM_FMT_CI,
            G_IM_SIZ_8B,
            0,
            tlut,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        );
    }

    #[test]
    fn decode_ci4_combines_palette_bank_with_each_nibble() {
        let mut tlut = vec![[0, 0, 0, 0]; 0x30];
        tlut[0x21] = [1, 3, 5, 7];
        tlut[0x2f] = [2, 4, 6, 8];
        assert_texture_row(
            &[0x1f],
            2,
            G_IM_FMT_CI,
            G_IM_SIZ_4B,
            2,
            tlut,
            &[1, 3, 5, 7, 2, 4, 6, 8],
        );
    }

    #[test]
    fn decode_ci4_pal16_load_uses_palette_local_indices() {
        // Fail-against-bug: G_LOADTLUT stores a 16-entry pal16 load in this
        // decoder as entries 0..15. The old CI4 arm added palette<<4 again,
        // indexed past this Vec for every nonzero bank, and returned magenta.
        let mut tlut = vec![[0, 0, 0, 0]; 16];
        tlut[1] = [11, 22, 33, 44];
        tlut[15] = [55, 66, 77, 88];
        assert_texture_row(
            &[0x1f],
            2,
            G_IM_FMT_CI,
            G_IM_SIZ_4B,
            2,
            tlut,
            &[11, 22, 33, 44, 55, 66, 77, 88],
        );
    }

    // --- Projection w-sign regression (the "giant triangles from a point"
    //     bug): the MVP must be applied as a ROW vector, not a column vector.

    /// Column-vector application of an asymmetric perspective·modelview MVP
    /// (`mvp · v`) computes the TRANSPOSE of the true transform and produces a
    /// huge, sign-flipping `w` -- the projection bug. `transform_point` must do
    /// the row-vector product (`v · mvp`) so `w ≈ -z_eye`, a sane depth.
    ///
    /// Matrices are the LIVE OoT gameplay task dump (decoded via `read_mtx`):
    /// perspective P with the projective term in row 2 col 3, and a modelview
    /// translation of (-53, -5, 0). Cited in `transform_point`'s doc comment.
    #[test]
    fn transform_point_row_vector_gives_sane_perspective_w() {
        // guPerspective output P (hardware [row][col], no transpose):
        let p: Mat4 = [
            [2.7990265, 0.0, 0.0, 0.0],
            [0.0, 3.7320404, 0.0, 0.0],
            [0.0, 0.0, -1.0015564, -1.0],
            [0.0, 0.0, -20.015625, 0.0],
        ];
        // Modelview: pure translation by (-53, -5, 0) (4th ROW, N64 layout).
        let mut m: Mat4 = identity();
        m[3][0] = -53.0;
        m[3][1] = -5.0;
        // mvp = modelview * (view*proj); here view is folded in, so M * P.
        let mvp = mat_mul(&m, &p);

        // Two object-space vertices of one small object (ob magnitudes ~10).
        // Under a correct row-vector transform their `w` is the SAME sane
        // depth (both at eye-z = -5 after the translate -> w = -z_eye = 5),
        // NOT the ±thousands sign-flipping garbage the transpose produced.
        for &(x, y, z) in &[(11.0, 0.0, -5.0), (5.0, 0.0, -5.0)] {
            let clip = transform_point(&mvp, x, y, z);
            let w = clip[3];
            // The true perspective depth for these verts is w = 5.0.
            assert!(
                (w - 5.0).abs() < 1e-3,
                "row-vector w should be the sane depth 5.0, got {w}"
            );
            assert!(w.abs() < 1e3, "w must be a small sane depth, got {w}");
        }

        // Guard against a regression to the column-vector form: assert the
        // BUGGED product (`mvp · v`, the old code) really did explode `w`.
        // This documents the failure mode so a reviewer sees the bug is real.
        let col_vec_w = {
            let v = [11.0f32, 0.0, -5.0, 1.0];
            // out[r] = sum_k mvp[r][k] * v[k] -- the OLD column-vector product.
            let mut s = 0.0;
            for k in 0..4 {
                s += mvp[3][k] * v[k];
            }
            s
        };
        assert!(
            col_vec_w.abs() > 1e3,
            "the column-vector (transposed) apply must produce the pathological \
             large w this test guards against; got {col_vec_w}"
        );
        // And it flips sign vs the second vertex (the "fan" signature).
        let col_vec_w2 = {
            let v = [5.0f32, 0.0, -5.0, 1.0];
            let mut s = 0.0;
            for k in 0..4 {
                s += mvp[3][k] * v[k];
            }
            s
        };
        assert!(
            col_vec_w.signum() == col_vec_w2.signum() && col_vec_w != col_vec_w2,
            "column-vector w varies wildly with x (the bug); w1={col_vec_w} w2={col_vec_w2}"
        );
    }

    /// A symmetric/diagonal matrix (all the reference fixtures) is unchanged
    /// by the row-vs-column swap (`m == m^T`), so the fix is transparent to
    /// the byte-exact goldens.
    #[test]
    fn transform_point_symmetric_matrix_unaffected_by_convention() {
        let mut m: Mat4 = identity();
        m[0][0] = 2.0;
        m[1][1] = 3.0;
        m[2][2] = 4.0;
        m[3][3] = 1.0;
        let clip = transform_point(&m, 5.0, 7.0, 9.0);
        assert_eq!(clip, [10.0, 21.0, 36.0, 1.0]);
    }

    /// Regression for the exact fixed-point `guLookAt` matrix observed in the
    /// Hyrule Field title-camera task. The writer trace establishes that
    /// `guLookAtF` receives eye `(-4000,-1,5228)`; its translation therefore
    /// is `(3263,694,5675) = -(eye · basis)`. Those translation values are
    /// camera-space coordinates of the world origin, not the world-space eye.
    ///
    /// Replacing them with `-translation · basis` (the discarded diagnostic
    /// transform) moves the camera to a different world-space eye. This test
    /// fails under that rewrite because the traced eye no longer maps to the
    /// view-space origin.
    #[test]
    fn hyrule_field_live_gu_look_at_translation_matches_traced_eye() {
        // Decoded from the 64-byte Mtx written at physical 0x1888c8. The
        // fixed-point quantization accounts for the small origin tolerance.
        let view: Mat4 = [
            [-0.3885498, 0.11167908, 0.9146271, 0.0],
            [-1.5258789e-5, 0.99261475, -0.12121582, 0.0],
            [-0.92141724, -0.04710388, -0.38568115, 0.0],
            [3262.9912, 694.052, 5674.783, 1.0],
        ];
        let eye = [-4000.0, -1.0, 5228.0];

        for (c, (((&basis_x, &basis_y), &basis_z), &translation)) in view[0]
            .iter()
            .zip(view[1].iter())
            .zip(view[2].iter())
            .zip(view[3].iter())
            .take(3)
            .enumerate()
        {
            let expected_translation = -(eye[0] * basis_x + eye[1] * basis_y + eye[2] * basis_z);
            assert!(
                (translation - expected_translation).abs() < 0.1,
                "translation[{c}] must be -(eye · basis[{c}]): got {}, expected {expected_translation}",
                translation
            );
        }

        let eye_in_view = transform_point(&view, eye[0], eye[1], eye[2]);
        for (axis, value) in eye_in_view[..3].iter().enumerate() {
            assert!(
                value.abs() < 0.1,
                "traced eye must map to the view-space origin; axis {axis} was {value}"
            );
        }
        assert!((eye_in_view[3] - 1.0).abs() < f32::EPSILON);
    }

    // --- Synthetic large-world projection regression ---------------------
    //
    // This synthetic scene has a camera at world ~(3000,700,5600) and an
    // object translated to ~-4000, so both sides carry LARGE world
    // coordinates. It drives the full decode path -- fixed-point `Mtx`
    // bytes (`read_mtx`) -> projection LOAD(persp) then PROJECTION|MUL(view)
    // -> modelview LOAD -> `recompute_mvp` (`M · (V · P)`) -> row-vector
    // `transform_point` -> an explicit 320x240 viewport map -- for the exact
    // large-world matrix shapes and asserts every vertex lands in-frustum
    // with a sane POSITIVE `w` (~ -z_eye ~= +7000), never the negative /
    // sign-flipping `w` and ±4000 screen-z of the mis-projection.
    //
    // The synthetic view is a proper `guLookAt` matrix: its translation row is
    // `-(eye · basis)` = (5419.7, -367.3, -3367.7), NOT the raw eye. That is
    // the load-bearing distinction -- feed the raw eye (3000,700,5600) into
    // row 3 instead and the origin vertex flips to `w = -1921` (behind the
    // camera). This asserts the decode+compose of a correct synthetic
    // large-world view/model/perspective chain.
    //
    // It fails against the historical transpose bug too: a re-introduced
    // `Mtx` transpose-on-read or a column-vector apply turns the asymmetric
    // large-world MVP into its transpose and collapses `w` to garbage.
    #[test]
    fn large_world_perspective_view_model_projects_in_frustum() {
        let mut rdram = vec![0u8; 0x8000];

        // guPerspective(fovy=60, aspect=4/3, near=100, far=12800), hardware
        // [row][col]: projective term [2][3]=-1, depth translate [3][2].
        let persp = [
            [1.299_038, 0.0, 0.0, 0.0],
            [0.0, 1.7320508, 0.0, 0.0],
            [0.0, 0.0, -1.015_748, -1.0],
            [0.0, 0.0, -201.574_8, 0.0],
        ];
        // PROPER guLookAt view: 3x3 = camera basis (right/up/look as columns),
        // translation ROW = -(eye · basis). Eye world ~(3000,700,5600) looking
        // toward (-4000,0,5200). (Raw eye in row 3 would be the bug.)
        let view = [
            [0.05704979, -0.09918146, 0.993_432_6, 0.0],
            [0.0, 0.99505322, 0.09934326, 0.0],
            [-0.998_371_3, -0.00566751, 0.05676758, 0.0],
            [5_419.73, -367.254_8, -3367.7366, 1.0],
        ];
        // Large-world object modelview: rot(15° about Y) then translate to
        // world (-4000, 0, 5200) -- asymmetric so `mvp != mvp^T`.
        let model = [
            [0.965_925_8, 0.0, -0.25881905, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.25881905, 0.0, 0.965_925_8, 0.0],
            [-4000.0, 0.0, 5200.0, 1.0],
        ];

        // Object-space vertices (small, ob magnitudes ~50).
        wr_vtx(&mut rdram, 0x3000, 0, 0, 0, [255, 0, 0, 255]);
        wr_vtx(&mut rdram, 0x3010, 50, 30, 0, [0, 255, 0, 255]);
        wr_vtx(&mut rdram, 0x3020, -50, 0, 40, [0, 0, 255, 255]);

        wr_mtx(&mut rdram, 0x2000, persp);
        wr_mtx(&mut rdram, 0x2100, view);
        wr_mtx(&mut rdram, 0x2200, model);
        wr_centered_viewport(&mut rdram, 0x2400);

        // G_MTX wire = params ^ G_MTX_PUSH:
        //   persp PROJECTION|LOAD        = 0x06 -> wire 0x07
        //   view  PROJECTION|MUL(NOPUSH) = 0x04 -> wire 0x05
        //   model LOAD (modelview)       = 0x02 -> wire 0x03
        let mtx_len = ((64u32 - 1) / 8) << 19;
        let mtx_cmd = |idx: u32| ((G_MTX as u32) << 24) | mtx_len | idx;
        let mut off = 0x1000;
        wr_cmd(&mut rdram, off, movemem_viewport_word(), 0x2400);
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x07), 0x2000); // persp LOAD
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x05), 0x2100); // view PROJECTION|MUL
        off += 8;
        wr_cmd(&mut rdram, off, mtx_cmd(0x03), 0x2200); // model LOAD
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
        off += 8;
        wr_cmd(
            &mut rdram,
            off,
            ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
            0,
        );
        off += 8;
        wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

        let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
        assert_eq!(tris.len(), 1, "expected one transformed triangle");

        // Every vertex must project to a sane POSITIVE depth ~7000 and land
        // inside the explicit 320x240 screen ([0,320] x [0,240]). The origin
        // vertex maps to screen center (~160, 120). The mis-projection gave
        // negative `w` and NDC well outside [-1,1] (pz swinging ±4000).
        for (i, v) in tris[0].v.iter().enumerate() {
            assert!(
                v.w > 1.0,
                "large-world vertex {i} must have a sane positive clip-w \
                 (~7000, = -z_eye), got w={} (negative/tiny w is the \
                 mis-projection this test guards)",
                v.w
            );
            assert!(
                (5000.0..9000.0).contains(&v.w),
                "large-world clip-w must be the coherent perspective depth \
                 (~7000), got w={} -- a decode transpose / wrong MVP order \
                 turns it into garbage",
                v.w
            );
            assert!(
                (0.0..=320.0).contains(&v.x) && (0.0..=240.0).contains(&v.y),
                "large-world vertex {i} must land inside the 320x240 screen, \
                 got ({}, {}) -- out-of-frustum is the ±4000 pz mis-projection",
                v.x,
                v.y
            );
        }
        // Origin vertex at screen center is the crisp anchor.
        let v0 = &tris[0].v[0];
        assert!(
            (v0.x - 160.0).abs() < 1.0 && (v0.y - 120.0).abs() < 1.0,
            "object-origin vertex must land at screen center (~160, ~120) \
             under the correct large-world MVP; got ({}, {})",
            v0.x,
            v0.y
        );
    }

    #[test]
    fn raw_rdp_range_rejects_truncated_edge_triangle_by_name() {
        let mut rdram = vec![0u8; 0x20];
        wr_cmd(&mut rdram, 0, 0x0800_0000, 0);
        let error = validate_raw_rdp_command_range(&rdram, 0, 8).unwrap_err();
        assert!(error.to_string().contains("truncated"));
        assert!(error.to_string().contains("RDP_TRI_FILL"));
    }

    #[test]
    fn raw_rdp_full_sync_inspection_distinguishes_absent_and_reached() {
        let mut rdram = vec![0u8; 16];
        wr_cmd(&mut rdram, 0, (G_RDPPIPESYNC as u32) << 24, 0);
        assert_eq!(
            raw_rdp_full_sync_status(&rdram, 0, 8).unwrap(),
            fn64_render::DpFullSyncStatus::NotReached
        );

        wr_cmd(&mut rdram, 8, (G_RDPFULLSYNC as u32) << 24, 0x1234_5678);
        assert_eq!(
            raw_rdp_full_sync_status(&rdram, 0, 16).unwrap(),
            fn64_render::DpFullSyncStatus::Reached
        );
    }

    #[test]
    fn raw_rdp_full_sync_inspection_skips_triangle_payload_words() {
        let mut rdram = vec![0u8; 32];
        wr_cmd(&mut rdram, 0, 0x0800_0000, 0);
        wr_cmd(&mut rdram, 8, (G_RDPFULLSYNC as u32) << 24, 0);

        assert_eq!(
            raw_rdp_full_sync_status(&rdram, 0, 32).unwrap(),
            fn64_render::DpFullSyncStatus::NotReached,
            "an opcode-shaped triangle coefficient is data, not a command"
        );
    }

    #[test]
    fn raw_rdp_full_sync_inspection_rejects_truncated_commands() {
        let mut rdram = vec![0u8; 8];
        wr_cmd(&mut rdram, 0, 0x0800_0000, 0);
        let error = raw_rdp_full_sync_status(&rdram, 0, 8).unwrap_err();
        assert!(error.to_string().contains("truncated"));
    }

    #[test]
    fn raw_rdp_unknown_opcode_records_returned_error() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut rdram = vec![0u8; 8];
        wr_cmd(&mut rdram, 0, 0x1000_0000, 0);

        let error = validate_raw_rdp_command_range(&rdram, 0, 8).unwrap_err();
        assert!(error.to_string().contains("G_<unrecognized>"));
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].operation, "render.gbi.raw-rdp.command");
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::ReturnedError
        );
        assert!(events[0].context.contains("0x10"));
    }

    #[test]
    fn raw_rdp_sync_commands_ignore_their_unassigned_second_word() {
        let mut rdram = vec![0u8; 40];
        for (index, opcode) in [G_RDPLOADSYNC, G_RDPPIPESYNC, G_RDPTILESYNC, G_RDPFULLSYNC]
            .into_iter()
            .enumerate()
        {
            wr_cmd(
                &mut rdram,
                index * 8,
                (opcode as u32) << 24,
                0x0616_181a_u32.wrapping_add(index as u32),
            );
        }
        wr_cmd(&mut rdram, 32, (G_ENDDL as u32) << 24, 0);

        validate_raw_rdp_command_range(&rdram, 0, 32).unwrap();
        let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], RenderOp::FullSync));
    }

    #[test]
    fn raw_rdp_edge_triangle_retains_signed_coefficients_through_render_op() {
        let mut rdram = vec![0u8; 40];
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let slope_major = (5.0f32 / 6.0 * 65536.0).round() as u32;
        let slope_low = (5.0f32 / 3.0 * 65536.0).round() as u32;
        wr_cmd(
            &mut rdram,
            0,
            0x0800_0000 | (1 << 23) | (2 << 19) | (3 << 16) | yl,
            (ym << 16) | yh,
        );
        wr_cmd(&mut rdram, 8, 1 << 16, slope_low);
        wr_cmd(&mut rdram, 16, 1 << 16, slope_major);
        wr_cmd(&mut rdram, 24, 1 << 16, 0);
        wr_cmd(&mut rdram, 32, (G_ENDDL as u32) << 24, 0);

        validate_raw_rdp_command_range(&rdram, 0, 32).unwrap();
        let coefficients = decode_rdp_edge_coefficients(&rdram, 0).unwrap();
        assert!(coefficients.right_major);
        assert_eq!((coefficients.level, coefficients.tile), (2, 3));
        assert_eq!(
            (coefficients.yh, coefficients.ym, coefficients.yl),
            (4, 16, 28)
        );
        let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
        let RenderOp::RawTriangle(triangle) = &ops[0] else {
            panic!("raw edge command did not emit a raw triangle")
        };
        assert_eq!(triangle.edge, coefficients);
        assert!(triangle.shade.is_none());
        assert!(triangle.texture_coefficients.is_none());
        assert!(triangle.z.is_none());
    }

    #[test]
    fn raw_rdp_z_triangle_retains_all_depth_coefficients() {
        let mut rdram = vec![0u8; 56];
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        wr_cmd(&mut rdram, 0, 0x0980_0000 | yl, (ym << 16) | yh);
        wr_cmd(
            &mut rdram,
            8,
            1 << 16,
            (5.0f32 / 3.0 * 65536.0).round() as u32,
        );
        wr_cmd(
            &mut rdram,
            16,
            1 << 16,
            (5.0f32 / 6.0 * 65536.0).round() as u32,
        );
        wr_cmd(&mut rdram, 24, 1 << 16, 0);
        wr_cmd(&mut rdram, 32, 2 << 16, 1 << 16);
        wr_cmd(&mut rdram, 40, 3 << 16, 0);
        wr_cmd(&mut rdram, 48, (G_ENDDL as u32) << 24, 0);

        validate_raw_rdp_command_range(&rdram, 0, 48).unwrap();
        assert_eq!(
            decode_rdp_z_coefficients(&rdram, 32),
            Some(RdpZCoefficients {
                z: 2 << 16,
                dzdx: 1 << 16,
                dzde: 3 << 16,
                dzdy: 0,
            })
        );
        let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
        let RenderOp::RawTriangle(triangle) = &ops[0] else {
            panic!("raw edge-plus-Z command did not emit a raw triangle")
        };
        assert_eq!(
            triangle.z,
            Some(RdpZCoefficients {
                z: 2 << 16,
                dzdx: 1 << 16,
                dzde: 3 << 16,
                dzdy: 0,
            })
        );
    }

    #[test]
    fn raw_rdp_shade_z_triangle_decodes_signed_component_gradients() {
        let mut rdram = vec![0u8; 120];
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        wr_cmd(&mut rdram, 0, 0x0d80_0000 | yl, (ym << 16) | yh);
        wr_cmd(
            &mut rdram,
            8,
            1 << 16,
            (5.0f32 / 3.0 * 65536.0).round() as u32,
        );
        wr_cmd(
            &mut rdram,
            16,
            1 << 16,
            (5.0f32 / 6.0 * 65536.0).round() as u32,
        );
        wr_cmd(&mut rdram, 24, 1 << 16, 0);
        wr_cmd(&mut rdram, 32, (10 << 16) | 20, (30 << 16) | 255);
        wr_cmd(&mut rdram, 40, u32::from(u16::MAX) << 16, 0);
        wr_cmd(&mut rdram, 48, 0, 0);
        wr_cmd(&mut rdram, 56, 0, 0);
        wr_cmd(&mut rdram, 64, 0, 0);
        wr_cmd(&mut rdram, 72, 2, 0);
        wr_cmd(&mut rdram, 80, 0, 0);
        wr_cmd(&mut rdram, 88, 0, 0);
        wr_cmd(&mut rdram, 96, 4 << 16, 0);
        wr_cmd(&mut rdram, 104, 0, 0);
        wr_cmd(&mut rdram, 112, (G_ENDDL as u32) << 24, 0);

        validate_raw_rdp_command_range(&rdram, 0, 112).unwrap();
        let shade = decode_rdp_shade_coefficients(&rdram, 32).unwrap();
        assert_eq!(shade.color, [10 << 16, 20 << 16, 30 << 16, 255 << 16]);
        assert_eq!(shade.dcdx, [-65536, 0, 0, 0]);
        assert_eq!(shade.dcdy, [0, 2 << 16, 0, 0]);
        let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
        let RenderOp::RawTriangle(triangle) = &ops[0] else {
            panic!("raw shade command did not emit a raw triangle")
        };
        assert_eq!(triangle.shade, Some(shade));
        assert_eq!(
            triangle.z,
            Some(RdpZCoefficients {
                z: 4 << 16,
                dzdx: 0,
                dzde: 0,
                dzdy: 0,
            })
        );
    }

    #[test]
    fn raw_rdp_texture_coefficients_preserve_signed_fixed_components() {
        let mut rdram = vec![0u8; 64];
        wr_cmd(&mut rdram, 0, (u32::from(u16::MAX - 1) << 16) | 3, 1 << 16);
        wr_cmd(&mut rdram, 8, (1 << 16) | u32::from(u16::MAX), 0);
        wr_cmd(&mut rdram, 16, 0x8000_0000, 0);
        wr_cmd(&mut rdram, 24, 0, 0);
        wr_cmd(&mut rdram, 32, 0, 0);
        wr_cmd(&mut rdram, 40, 0, 0);
        wr_cmd(&mut rdram, 48, 0, 0);
        wr_cmd(&mut rdram, 56, 0, 0);

        assert_eq!(
            decode_rdp_texture_coefficients(&rdram, 0),
            Some(RdpTextureCoefficients {
                stw: [-(2 << 16) + 0x8000, 3 << 16, 1 << 16],
                dstdx: [1 << 16, -65536, 0],
                dstde: [0; 3],
                dstdy: [0; 3],
            })
        );
    }

    #[test]
    fn raw_rdp_triangle_widths_cover_every_coefficient_variant() {
        assert_eq!(
            (0x08..=0x0f)
                .map(|opcode| raw_rdp_command_width(opcode).unwrap())
                .collect::<Vec<_>>(),
            [32, 48, 96, 112, 96, 112, 160, 176]
        );
    }

    #[test]
    fn raw_rdp_triangle_accepts_flagged_six_bit_wire_opcode() {
        let mut rdram = vec![0u8; 176];
        wr_cmd(&mut rdram, 0, 0xcf00_0000, 0);

        validate_raw_rdp_command_range(&rdram, 0, 176).unwrap();
        assert_eq!(raw_rdp_command_width(0xcf), Some(176));
        assert_eq!(raw_rdp_opcode_name(0xcf & 0x3f), "RDP_TRI_SHADE_TXTR_ZBUFF");
    }

    #[test]
    fn raw_rdp_range_accepts_depth_image_register_command() {
        let mut rdram = vec![0u8; 8];
        wr_cmd(&mut rdram, 0, 0xfe00_0000, 0x0000_0400);
        validate_raw_rdp_command_range(&rdram, 0, 8).unwrap();
    }
}
