//! Device-free microcode identity derived from pinned MIT RT64's public
//! recognition database. Identity does not admit HLE or establish silicon
//! behavior; it prevents a private task image from choosing its own family.

use crate::{TaskAdmissionRawWindow, TaskAdmissionRawWindowSize, UcodeId};
use fn64_runtime::RdramAddr;
use twox_hash::xxhash3_64::Hasher;

/// Exact pinned MIT source whose public recognition rows define this identity.
pub const F3DZEX2_IDENTITY_SOURCE: &str =
    "rt64@f0728a2520d5aa735886240de3fee75cc805f6d6:src/gbi/rt64_gbi.cpp";

// `rt64_gbi.cpp` (SHA-256 of the whole file,
// `ecb1dc7bb915576ba2fe4116fee0c8de9d4c696e2d5f6c87c67491335e40ae53`, matching
// `docs/rt64-port-inventory.json`'s `sources.port.sha256` -- identical to
// `sources.oracle.sha256` for this file, so the digest is simultaneously
// both) is a partial port: only the six `textSegments`/`dataSegments` XXH3
// row constants for the three F3DZEX2 NON-FIFO variants fn64 recognizes
// (2.06H/2.08I/2.08J) are carried, copied verbatim from that file's
// `GBISegment` tables; the surrounding classification code, the other ~90
// text/data rows for other microcode families, and `GBIManager`'s broader
// deduction logic are not ported.

/// Pinned RT64 F3DZEX2 variant recognized from one raw text/data pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum F3dzex2Variant {
    NoNFifo206H,
    NoNFifo208I,
    NoNFifo208J,
}

impl F3dzex2Variant {
    pub const fn family(self) -> UcodeId {
        UcodeId::F3dzex2
    }

    /// Stable admission-wire tag. These values are append-only because they
    /// participate in canonical task-plan identities and the native ABI.
    pub const fn canonical_tag(self) -> u32 {
        match self {
            Self::NoNFifo206H => 1,
            Self::NoNFifo208I => 2,
            Self::NoNFifo208J => 3,
        }
    }

    pub const fn point_lighting(self) -> bool {
        matches!(self, Self::NoNFifo208I | Self::NoNFifo208J)
    }

    pub const fn no_near_clip(self) -> bool {
        true
    }
}

/// Largest raw prefix required by pinned RT64's F3DZEX2 recognition rows.
pub const F3DZEX2_RAW_WINDOW_SIZE: TaskAdmissionRawWindowSize = TaskAdmissionRawWindowSize {
    text: 0x1630,
    data: 0x420,
};

#[derive(Clone, Copy)]
struct IdentityRow {
    variant: F3dzex2Variant,
    text_bytes: usize,
    text_xxh3: u64,
    data_bytes: usize,
    data_xxh3: u64,
}

// Pinned MIT RT64 `textSegments` and `dataSegments`. A row is authoritative
// only as one intersecting text/data identity; cross-pairing never classifies.
const F3DZEX2_ROWS: [IdentityRow; 3] = [
    IdentityRow {
        variant: F3dzex2Variant::NoNFifo206H,
        text_bytes: 0x1390,
        text_xxh3: 0x1a24_186a_d41d_2568,
        data_bytes: 0x420,
        data_xxh3: 0xe3e5_c20b_c750_105e,
    },
    IdentityRow {
        variant: F3dzex2Variant::NoNFifo208I,
        text_bytes: 0x1630,
        text_xxh3: 0xf5ee_0949_f308_cfe3,
        data_bytes: 0x420,
        data_xxh3: 0x002d_7fa2_54ab_d8e7,
    },
    IdentityRow {
        variant: F3dzex2Variant::NoNFifo208J,
        text_bytes: 0x1630,
        text_xxh3: 0x7502_444d_3ddb_d4bf,
        data_bytes: 0x420,
        data_xxh3: 0x6069_a280_3cb3_9e66,
    },
];

/// Copy raw native-word backing-store bytes without applying logical N64 byte
/// lane conversion. Pinned RT64 hashes these exact storage bytes.
pub fn capture_task_admission_raw_window(
    rdram: &[u8],
    text_address: RdramAddr,
    data_address: RdramAddr,
    size: TaskAdmissionRawWindowSize,
) -> Option<TaskAdmissionRawWindow> {
    let text_start = text_address.offset() as usize;
    let data_start = data_address.offset() as usize;
    let text_end = text_start.checked_add(size.text)?;
    let data_end = data_start.checked_add(size.data)?;
    Some(TaskAdmissionRawWindow {
        text: rdram.get(text_start..text_end)?.to_vec(),
        data: rdram.get(data_start..data_end)?.to_vec(),
    })
}

fn identify_from_rows(
    window: &TaskAdmissionRawWindow,
    rows: &[IdentityRow],
) -> Option<F3dzex2Variant> {
    let mut matched = rows.iter().filter(|row| {
        let Some(text) = window.text.get(..row.text_bytes) else {
            return false;
        };
        let Some(data) = window.data.get(..row.data_bytes) else {
            return false;
        };
        Hasher::oneshot(text) == row.text_xxh3 && Hasher::oneshot(data) == row.data_xxh3
    });
    let variant = matched.next()?.variant;
    matched.next().is_none().then_some(variant)
}

/// Classify one raw task-entry pair against the three pinned F3DZEX2 rows.
/// Unknown, short, cross-paired, or ambiguous windows return `None`.
pub fn identify_f3dzex2(window: &TaskAdmissionRawWindow) -> Option<F3dzex2Variant> {
    identify_from_rows(window, &F3DZEX2_ROWS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::RdramViewMut;

    #[test]
    fn xxh3_matches_public_default_secret_vectors() {
        assert_eq!(Hasher::oneshot(&[]), 0x2d06_8005_38d3_94c2);
        let bytes = (0..1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        assert_eq!(Hasher::oneshot(&bytes), 0xe5d7_8baf_a45b_2aa5);
    }

    #[test]
    fn f3dzex2_variant_tags_bind_non_and_point_lighting_capabilities() {
        assert_eq!(F3dzex2Variant::NoNFifo206H.canonical_tag(), 1);
        assert_eq!(F3dzex2Variant::NoNFifo208I.canonical_tag(), 2);
        assert_eq!(F3dzex2Variant::NoNFifo208J.canonical_tag(), 3);
        assert!(!F3dzex2Variant::NoNFifo206H.point_lighting());
        assert!(F3dzex2Variant::NoNFifo208I.point_lighting());
        assert!(F3dzex2Variant::NoNFifo208J.point_lighting());
        assert!(F3dzex2Variant::NoNFifo206H.no_near_clip());
        assert!(F3dzex2Variant::NoNFifo208I.no_near_clip());
        assert!(F3dzex2Variant::NoNFifo208J.no_near_clip());
    }

    #[test]
    fn classifier_requires_one_intersecting_text_data_row() {
        let text_a = vec![0x11; 257];
        let text_b = vec![0x22; 263];
        let data_a = vec![0x31; 67];
        let data_b = vec![0x42; 71];
        let rows = [
            IdentityRow {
                variant: F3dzex2Variant::NoNFifo206H,
                text_bytes: text_a.len(),
                text_xxh3: Hasher::oneshot(&text_a),
                data_bytes: data_a.len(),
                data_xxh3: Hasher::oneshot(&data_a),
            },
            IdentityRow {
                variant: F3dzex2Variant::NoNFifo208I,
                text_bytes: text_b.len(),
                text_xxh3: Hasher::oneshot(&text_b),
                data_bytes: data_b.len(),
                data_xxh3: Hasher::oneshot(&data_b),
            },
        ];

        assert_eq!(
            identify_from_rows(
                &TaskAdmissionRawWindow {
                    text: text_a.clone(),
                    data: data_a.clone(),
                },
                &rows,
            ),
            Some(F3dzex2Variant::NoNFifo206H)
        );
        assert_eq!(
            identify_from_rows(
                &TaskAdmissionRawWindow {
                    text: text_a,
                    data: data_b,
                },
                &rows,
            ),
            None
        );
        assert_eq!(
            identify_from_rows(
                &TaskAdmissionRawWindow {
                    text: text_b[..200].to_vec(),
                    data: data_a,
                },
                &rows,
            ),
            None
        );
    }

    #[test]
    fn raw_window_preserves_native_storage_byte_order() {
        let mut storage = vec![0u8; 32];
        RdramViewMut::from_storage(&mut storage).write_u32(RdramAddr::from_offset(8), 0x0123_4567);
        let window = capture_task_admission_raw_window(
            &storage,
            RdramAddr::from_offset(8),
            RdramAddr::from_offset(12),
            TaskAdmissionRawWindowSize { text: 4, data: 4 },
        )
        .unwrap();

        if cfg!(target_endian = "little") {
            assert_eq!(window.text, [0x67, 0x45, 0x23, 0x01]);
            assert_ne!(
                Hasher::oneshot(&window.text),
                Hasher::oneshot(&[1, 0x23, 0x45, 0x67])
            );
        } else {
            assert_eq!(window.text, [0x01, 0x23, 0x45, 0x67]);
            assert_eq!(
                Hasher::oneshot(&window.text),
                Hasher::oneshot(&[1, 0x23, 0x45, 0x67])
            );
        }
    }

    #[test]
    fn pinned_rows_are_unique_and_short_windows_do_not_classify() {
        for (index, row) in F3DZEX2_ROWS.iter().enumerate() {
            assert!(F3DZEX2_ROWS[index + 1..].iter().all(|other| {
                (row.text_bytes, row.text_xxh3, row.data_bytes, row.data_xxh3)
                    != (
                        other.text_bytes,
                        other.text_xxh3,
                        other.data_bytes,
                        other.data_xxh3,
                    )
            }));
        }
        assert_eq!(
            identify_f3dzex2(&TaskAdmissionRawWindow {
                text: vec![0; F3DZEX2_RAW_WINDOW_SIZE.text - 1],
                data: vec![0; F3DZEX2_RAW_WINDOW_SIZE.data - 1],
            }),
            None
        );
    }
}
