use fn64_render::{
    GeometryUcodeProfile, MicrocodeDataImageIdentity, RenderError, TaskAdmissionGeneration,
    TaskAdmissionSource, UcodeId,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _};
use super::*;
use super::types::*;
use super::matrix::*;
use super::tmem::*;
use super::state::*;
use super::entries::*;
use super::stream::*;
use super::geometry::*;

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
pub(super) const G_NOOP: u8 = 0x00;
pub(super) const G_SPNOOP: u8 = 0xE0;
pub(super) const G_RDPHALF_2: u8 = 0xF1;

pub(super) const LEGACY_G_SPNOOP: u8 = 0x00;
pub(super) const L3DEX_G_MTX: u8 = 0x01;
pub(super) const L3DEX_G_MOVEMEM: u8 = 0x03;
pub(super) const L3DEX_G_VTX: u8 = 0x04;
pub(super) const L3DEX_G_DL: u8 = 0x06;
pub(super) const F3DEX_G_LOAD_UCODE: u8 = 0xaf;
pub(super) const F3DEX_G_BRANCH_Z: u8 = 0xb0;
pub(super) const F3DEX_G_TRI2: u8 = 0xb1;
pub(super) const F3DEX_G_MODIFYVTX: u8 = 0xb2;
pub(super) const LEGACY_G_RDPHALF_2: u8 = 0xb3;
pub(super) const LEGACY_G_RDPHALF_1: u8 = 0xb4;
pub(super) const L3DEX_G_LINE3D: u8 = 0xb5;
pub(super) const L3DEX_G_CLEARGEOMETRYMODE: u8 = 0xb6;
pub(super) const L3DEX_G_SETGEOMETRYMODE: u8 = 0xb7;
pub(super) const L3DEX_G_ENDDL: u8 = 0xb8;
pub(super) const L3DEX_G_SETOTHERMODE_L: u8 = 0xb9;
pub(super) const L3DEX_G_SETOTHERMODE_H: u8 = 0xba;
pub(super) const L3DEX_G_TEXTURE: u8 = 0xbb;
pub(super) const L3DEX_G_MOVEWORD: u8 = 0xbc;
pub(super) const L3DEX_G_POPMTX: u8 = 0xbd;
pub(super) const LEGACY_G_CULLDL: u8 = 0xbe;
pub(super) const L3DEX_G_TRI1: u8 = 0xbf;
pub(super) const L3DEX_G_NOOP: u8 = 0xc0;

pub(super) const LEGACY_G_MW_POINTS: u32 = 0x0c;
pub(super) const LEGACY_G_CLIPPING: u32 = 0x0080_0000;
pub(super) const LEGACY_G_SHADING_SMOOTH: u32 = 0x0000_0200;
pub(super) const LEGACY_G_CULL_FRONT: u32 = 0x0000_1000;
pub(super) const LEGACY_G_CULL_BACK: u32 = 0x0000_2000;

pub(super) fn normalize_legacy_geometry_mode(family: GeometryWireFamily, word: u32) -> u32 {
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

pub(super) fn normalize_legacy_triangle_word(family: GeometryWireFamily, packed: u32) -> u32 {
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
pub(super) fn normalize_geometry_command(
    family: GeometryWireFamily,
    wire_w0: u32,
    wire_w1: u32,
    command_pc: usize,
) -> (u32, u32) {
    if matches!(
        family,
        GeometryWireFamily::F3dex2
            | GeometryWireFamily::F3dex2NoN
            | GeometryWireFamily::F3dex2Rej
            | GeometryWireFamily::F3dlx2Rej
            | GeometryWireFamily::F3dzex2
            | GeometryWireFamily::L3dex2
    ) {
        return (wire_w0, wire_w1);
    }
    let opcode = (wire_w0 >> 24) as u8;
    match opcode {
        LEGACY_G_SPNOOP => {
            assert_eq!(
                wire_w0,
                0,
                "{} G_SPNOOP command word must be zero",
                family.name()
            );
            assert_eq!(
                wire_w1,
                0,
                "{} G_SPNOOP second word must be zero",
                family.name()
            );
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
            let new_params =
                ((old_params & 0x01) << 2) | (old_params & 0x02) | ((old_params & 0x04) >> 2);
            let wire_index = new_params ^ 0x01;
            (
                (u32::from(G_MTX) << 24) | (7 << 19) | u32::from(wire_index),
                wire_w1,
            )
        }
        L3DEX_G_MOVEMEM => {
            let index = ((wire_w0 >> 16) & 0xff) as u8;
            let length = wire_w0 & 0xffff;
            assert_eq!(
                length,
                16,
                "{} G_MOVEMEM must carry one 16-byte record",
                family.name()
            );
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
                    (1..=32).contains(&n) && length & 0x03ff == (n as u32 * VTX_STRIDE as u32 - 1),
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
                (u32::from(G_VTX) << 24) | ((n as u32) << 12) | (((v0 + n) as u32) << 1),
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
                let encoded = [
                    ((wire_w1 >> 16) & 0xff) as usize,
                    ((wire_w1 >> 8) & 0xff) as usize,
                ];
                assert!(
                    encoded
                        .iter()
                        .all(|value| value.is_multiple_of(10) && *value <= 150),
                    "Fast3D G_LINE3D endpoints must use public v*10 packing: {encoded:?}"
                );
                let mut slots = [encoded[0] / 10, encoded[1] / 10];
                if flag == 1 {
                    slots.swap(0, 1);
                }
                slots
            } else {
                assert_eq!(wire_w1 >> 24, 0, "L3DEX G_LINE3D reserves w1[31:24]");
                let encoded = [
                    ((wire_w1 >> 16) & 0xff) as usize,
                    ((wire_w1 >> 8) & 0xff) as usize,
                ];
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
                assert_eq!(
                    wire_w0 & 0x00ff_0000,
                    0,
                    "{} G_CULLDL reserves w0[23:16]",
                    family.name()
                );
                assert_eq!(
                    wire_w1 & 0xffff_0000,
                    0,
                    "{} G_CULLDL reserves w1[31:16]",
                    family.name()
                );
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
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_TRI1 reserves its first-word payload",
                family.name()
            );
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
        F3DEX_G_BRANCH_Z if family.uses_legacy_polygon_wire() => (
            (u32::from(G_BRANCH_Z) << 24) | (wire_w0 & 0x00ff_ffff),
            wire_w1,
        ),
        F3DEX_G_LOAD_UCODE if family.is_legacy_loadable() => (
            (u32::from(G_LOAD_UCODE) << 24) | (wire_w0 & 0x00ff_ffff),
            wire_w1,
        ),
        LEGACY_G_RDPHALF_1 => (u32::from(G_RDPHALF_1) << 24, wire_w1),
        LEGACY_G_RDPHALF_2 => (u32::from(G_RDPHALF_2) << 24, wire_w1),
        L3DEX_G_CLEARGEOMETRYMODE => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_CLEARGEOMETRYMODE payload must be zero",
                family.name()
            );
            let clear = normalize_legacy_geometry_mode(family, wire_w1);
            (
                (u32::from(G_GEOMETRYMODE) << 24) | ((!clear) & 0x00ff_ffff),
                0,
            )
        }
        L3DEX_G_SETGEOMETRYMODE => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_SETGEOMETRYMODE payload must be zero",
                family.name()
            );
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
            assert_eq!(
                wire_w1,
                0,
                "{} G_ENDDL reserved second word must be zero",
                family.name()
            );
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
                (u32::from(normalized_opcode) << 24) | ((32 - shift - length) << 8) | (length - 1),
                wire_w1,
            )
        }
        L3DEX_G_TEXTURE => {
            let on = wire_w0 & 0xff;
            assert!(
                matches!(on, 0 | 1),
                "{} G_TEXTURE on={on} is outside G_OFF/G_ON",
                family.name()
            );
            (
                (u32::from(G_TEXTURE) << 24) | (wire_w0 & 0x00ff_ff00) | (on << 1),
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
                    (u32::from(G_MODIFYVTX) << 24) | (where_field << 16) | (slot * 2),
                    wire_w1,
                );
            }
            let (offset, data) = if index == u32::from(G_MW_NUMLIGHT) {
                assert_eq!(
                    offset,
                    0,
                    "{} G_MW_NUMLIGHT offset must be zero",
                    family.name()
                );
                assert!(
                    wire_w1 & 0x8000_0000 != 0 && (wire_w1 & 0x7fff_ffff).is_multiple_of(32),
                    "{} G_MW_NUMLIGHT data is not public ((n+1)*32)|0x80000000 packing",
                    family.name()
                );
                let n = (wire_w1 & 0x7fff_ffff) / 32;
                assert!(
                    (2..=8).contains(&n),
                    "{} G_MW_NUMLIGHT count is outside 1..=7",
                    family.name()
                );
                (offset, (n - 1) * 24)
            } else if index == u32::from(G_MW_LIGHTCOL) {
                assert!(
                    offset.is_multiple_of(4),
                    "{} G_MW_LIGHTCOL offset must be word aligned",
                    family.name()
                );
                let light = offset / 0x20;
                let copy = offset % 0x20;
                assert!(
                    light < 8 && matches!(copy, 0 | 4),
                    "{} G_MW_LIGHTCOL offset {offset:#06x} is outside public light colors",
                    family.name()
                );
                (light * 0x18 + copy, wire_w1)
            } else {
                (offset, wire_w1)
            };
            ((u32::from(G_MOVEWORD) << 24) | (index << 16) | offset, data)
        }
        L3DEX_G_POPMTX => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_POPMTX payload must be zero",
                family.name()
            );
            assert_eq!(
                wire_w1,
                0,
                "{} G_POPMTX supports only G_MTX_MODELVIEW",
                family.name()
            );
            (u32::from(G_POPMTX) << 24, 64)
        }
        L3DEX_G_NOOP => {
            assert_eq!(
                wire_w0 & 0x00ff_ffff,
                0,
                "{} G_NOOP payload must be zero",
                family.name()
            );
            assert_eq!(
                wire_w1,
                0,
                "{} G_NOOP second word must be zero",
                family.name()
            );
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

pub(super) fn consume_line_triangle_noop(family: GeometryWireFamily, wire_w0: u32, wire_w1: u32) -> bool {
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
pub(super) const G_MW_SEGMENT: u16 = 0x06;

/// `G_MV_VIEWPORT` (gbi.h) -- the `G_MOVEMEM` index that DMAs a `Vp`
/// (viewport scale/translate) struct into RSP state (F3DEX2-CONCEPTS.md
/// §1.4/§3.5).
pub(super) const G_MV_VIEWPORT: u8 = 8;
/// Destination used by the first half of public F3DEX2 `gSPForceMatrix` to
/// DMA an already-concatenated model/projection matrix into RSP state.
pub(super) const G_MV_MATRIX: u8 = 14;

// `gSPModifyVertex` destinations from the public F3DEX2 manual. These are
// final post-transform cache fields; the command does not re-run lighting or
// matrix projection.
pub(super) const G_MWO_POINT_RGBA: u8 = 0x10;
pub(super) const G_MWO_POINT_ST: u8 = 0x14;
pub(super) const G_MWO_POINT_XYSCREEN: u8 = 0x18;
pub(super) const G_MWO_POINT_ZSCREEN: u8 = 0x1C;

// --- F3DEX2 geometry-mode bits (F3DEX2-CONCEPTS.md §2.4) -----------------
/// Cull front-facing triangles.
pub(super) const G_CULL_FRONT: u32 = 0x0000_0200;
/// Cull back-facing triangles (the common case).
pub(super) const G_CULL_BACK: u32 = 0x0000_0400;
/// Enable vertex lighting. When set, a vertex's `cn[0..3]` bytes are a signed
/// s8 NORMAL (x,y,z), not an RGB color -- the vertex color is COMPUTED from
/// the loaded lights (ambient + per-directional N·L·color) instead of taken
/// from `cn` (`F3DEX2-CONCEPTS.md` §2.4; OoT gbi.h `G_LIGHTING`). Reading the
/// normal bytes as a flat color (the pre-lighting path) produced the
/// characteristic "rainbow fan" -- signed normal components reinterpreted as
/// unsigned color channels.
pub(super) const G_LIGHTING: u32 = 0x0002_0000;
/// Generate the vertex alpha coordinate from projected depth and the current
/// signed fog multiplier/offset instead of preserving the source vertex alpha.
pub(super) const G_FOG: u32 = 0x0001_0000;
/// Generate texture S/T from the signed vertex normal projected onto the
/// two screen-space directions loaded by `gSPLookAt`.
pub(super) const G_TEXTURE_GEN: u32 = 0x0004_0000;
/// Select the inverse-cosine texture-generation mapping. Public F3DEX2 uses
/// this together with [`G_TEXTURE_GEN`]; on its own it does not consume the
/// vertex normal or replace explicit texture coordinates.
pub(super) const G_TEXTURE_GEN_LINEAR: u32 = 0x0008_0000;
/// Interpolate endpoint shade attributes instead of using the first encoded
/// endpoint selected by the line command's flat-shading flag.
pub(super) const G_SHADING_SMOOTH: u32 = 0x0020_0000;

// --- F3DEX2 lighting: G_MOVEMEM/G_MOVEWORD indices + Light layout --------
/// `G_MV_LIGHT` (OoT gbi.h:1169) -- the `G_MOVEMEM` index that DMAs a `Light`
/// struct (diffuse color + direction, or an ambient color) into an RSP light
/// slot. F3DEX2 `gsSPLight` (gbi.h:2911) encodes `idx = G_MV_LIGHT` in the
/// w0 low byte and `ofs = n*24 + 24` (÷8 in the wire) in `field(w0,8,8)`.
pub(super) const G_MV_LIGHT: u8 = 0x0a;
/// `G_MW_NUMLIGHT` (OoT gbi.h:1210) -- the `G_MOVEWORD` index that sets the
/// directional-light count. F3DEX2 `gsSPNumLights` (gbi.h:2887) writes
/// `NUML(n) = n*24` as the data word, so `numDirectional = w1 / 24`. The
/// AMBIENT light is the slot AFTER the directional ones (gbi.h:2902 note:
/// "the highest numbered light is always the ambient light").
pub(super) const G_MW_NUMLIGHT: u16 = 0x02;
/// Public `gSPClipRatio` state block. Four writes select the negative/positive
/// X/Y clipping rectangle coefficients independently.
pub(super) const G_MW_CLIP: u16 = 0x04;
pub(super) const G_MWO_CLIP_RNX: u16 = 0x04;
pub(super) const G_MWO_CLIP_RNY: u16 = 0x0c;
pub(super) const G_MWO_CLIP_RPX: u16 = 0x14;
pub(super) const G_MWO_CLIP_RPY: u16 = 0x1c;
/// `G_MW_FOG` packs signed `fm`/`fo` factors in the high/low halfwords.
pub(super) const G_MW_FOG: u16 = 0x08;
/// `G_MW_LIGHTCOL` updates one of the two RGB copies in a light slot without
/// changing its direction. Public F3DEX2 `gbi.h` assigns each light a 24-byte
/// DMEM stride and exposes word offsets 0/4 within that stride.
pub(super) const G_MW_LIGHTCOL: u16 = 0x0a;
/// Second half of public F3DEX2 `gSPForceMatrix`. The header macro writes
/// `0x0001_0000` at offset zero after the `G_MV_MATRIX` DMA.
pub(super) const G_MW_FORCEMTX: u16 = 0x0c;
/// Public `.16` perspective-normalization scale written at offset zero.
pub(super) const G_MW_PERSPNORM: u16 = 0x0e;
/// One `Light_t` on the wire is 16 bytes (OoT gbi.h:1311 -- `col[3]`, pad,
/// `colc[3]`, pad, `dir[3]`, pad), padded to a 16-byte `Light` union.
pub(super) const LIGHT_STRIDE: usize = 16;
/// Max simultaneous lights F3DEX2 supports (7 directional + 1 ambient).
pub(super) const MAX_LIGHTS: usize = 8;

// --- Additional F3DEX2 opcode bytes. Reserved/unsupported encodings are
// named so their loud traps report the public command identity.
pub(super) const G_MODIFYVTX: u8 = 0x02;
pub(super) const G_CULLDL: u8 = 0x03;
pub(super) const G_BRANCH_Z: u8 = 0x04;
pub(super) const G_LINE3D: u8 = 0x08;
pub(super) const G_SPECIAL_1: u8 = 0xD5;
pub(super) const G_SPECIAL_2: u8 = 0xD4;
pub(super) const G_SPECIAL_3: u8 = 0xD3;
pub(super) const G_DMA_IO: u8 = 0xD6;
pub(super) const G_LOAD_UCODE: u8 = 0xDD;
/// Public `OSTask` guidance fixes task microcode text at `SP_UCODE_SIZE`;
/// the documented value is one complete 4 KiB IMEM bank. `gSPLoadUcodeEx`
/// carries only the data-section size because the text transfer has this
/// fixed size.
pub(super) const SP_UCODE_SIZE: usize = fn64_runtime::RSP_MEMORY_BANK_SIZE;
/// `G_TEXRECT` / `G_TEXRECTFLIP` (gbi.h:126-127). The raw RDP command is two
/// 64-bit words (16 bytes). `gSPTextureRectangle` wraps the second word in two
/// family-specific `G_RDPHALF_*` commands, making three `Gfx` entries. The
/// ordered decoder consumes either public form; the reference executor
/// implements non-flipped copy plus one/two-cycle normal and flipped
/// combiner/blender paths. Flipped copy remains a named gap.
pub(super) const G_TEXRECT: u8 = 0xE4;
pub(super) const G_TEXRECTFLIP: u8 = 0xE5;
/// F3DEX2 staging word used by compound commands. `G_BRANCH_Z` consumes it as
/// the conditional branch target; `G_LOAD_UCODE` uses the same wire opcode
/// for its data address.
pub(super) const G_RDPHALF_1: u8 = 0xE1;

/// Decode the second half of a public texture-rectangle command. Public
/// `gbi.h` exposes two wire forms: `gDPTextureRectangle` appends one raw RDP
/// word, while the display-list-safe `gSPTextureRectangle` wraps that word in
/// the family's `G_RDPHALF_1`/`G_RDPHALF_2` command envelope. The exact task
/// text selects which envelope is legal; opcode inspection never selects a
/// microcode family.
pub(super) fn decode_texture_rectangle_continuation(
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
pub(super) const G_RDPLOADSYNC: u8 = 0xE6;
pub(super) const G_RDPPIPESYNC: u8 = 0xE7;
pub(super) const G_RDPTILESYNC: u8 = 0xE8;
pub(super) const G_RDPFULLSYNC: u8 = 0xE9;
pub(super) const G_SETOTHERMODE_L: u8 = 0xE2;
pub(super) const G_SETOTHERMODE_H: u8 = 0xE3;
/// Full RDP other-mode write (`gsDPSetOtherMode`; gbi.h:3724-3737).
pub(super) const G_RDPSETOTHERMODE: u8 = 0xEF;
pub(super) const G_SETSCISSOR: u8 = 0xED;
pub(super) const G_SETCONVERT: u8 = 0xEC;
pub(super) const G_SETKEYR: u8 = 0xEB;
pub(super) const G_SETKEYGB: u8 = 0xEA;
pub(super) const G_SETPRIMDEPTH: u8 = 0xEE;
pub(super) const G_LOADTLUT: u8 = 0xF0;
pub(super) const G_SETTILESIZE: u8 = 0xF2;
pub(super) const G_LOADBLOCK: u8 = 0xF3;
pub(super) const G_LOADTILE: u8 = 0xF4;
pub(super) const G_SETTILE: u8 = 0xF5;
pub(super) const G_FILLRECT: u8 = 0xF6;
pub(super) const G_SETFILLCOLOR: u8 = 0xF7;
pub(super) const G_SETFOGCOLOR: u8 = 0xF8;
pub(super) const G_SETBLENDCOLOR: u8 = 0xF9;
pub(super) const G_SETPRIMCOLOR: u8 = 0xFA;
pub(super) const G_SETENVCOLOR: u8 = 0xFB;
pub(super) const G_SETCOMBINE: u8 = 0xFC;
pub(super) const G_SETTIMG: u8 = 0xFD;
pub(super) const G_SETZIMG: u8 = 0xFE;
pub(super) const G_SETCIMG: u8 = 0xFF;
