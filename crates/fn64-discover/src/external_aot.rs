//! Exact catalog validation for captured executable images compiled as static
//! AOT banks.
//!
//! Reproducibility of each capture group is established by [`crate::trace`].
//! This module proves the cross-image geometry needed by a static artifact:
//! every image owns at least one caller-supplied entry, image ranges do not
//! overlap, and their content-derived bank identities collide with neither one
//! another nor an immutable ROM-backed bank.

use crate::trace::{parse_executable_image_capture, ExecutableImageCapture, NormalizedRomDigest};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImmutableAotRange {
    pub label: String,
    pub bank_id: u64,
    pub va_start: u32,
    pub va_end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAotImage {
    pub capture: ExecutableImageCapture,
    pub bank_id: u64,
    pub owned_entries: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalAotCatalogError {
    EmptyCatalog,
    EmptyAllowedEntries,
    UnalignedAllowedEntry(u32),
    RomIdentityMismatch {
        image_id: String,
        generation: u64,
    },
    InvalidImageGeometry {
        image_id: String,
        generation: u64,
    },
    DuplicateImageIdentity {
        image_id: String,
        generation: u64,
    },
    ImageOwnsNoAllowedEntry {
        image_id: String,
        generation: u64,
    },
    FirstExecutedPcIsNotOwnedEntry {
        image_id: String,
        generation: u64,
        first_executed_pc: u32,
    },
    OverlappingImages {
        first_image_id: String,
        first_generation: u64,
        second_image_id: String,
        second_generation: u64,
    },
    OverlapsImmutableRange {
        image_id: String,
        generation: u64,
        immutable_label: String,
    },
    InvalidImmutableRange {
        immutable_label: String,
    },
    BankIdCollision {
        bank_id: u64,
    },
}

/// Content identity shared by external-image admission and transfer scans.
pub fn external_aot_bank_id(normalized_rom_sha256: &str, capture: &ExecutableImageCapture) -> u64 {
    let identity = format!(
        "fn64:executable-image:v1:{normalized_rom_sha256}:{}:{}:{}",
        capture.image_id, capture.generation, capture.sha256
    );
    let digest = Sha256::digest(identity.as_bytes());
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

fn overlaps(left_start: u32, left_end: u32, right_start: u32, right_end: u32) -> bool {
    left_start < right_end && right_start < left_end
}

/// Validate and deterministically order external executable images for AOT.
///
/// `immutable_ranges` must name every ROM-backed code bank installed beside
/// the result. A 64-bit content identity is not assumed collision-free: every
/// collision is rejected explicitly before generated code is emitted.
pub fn build_external_aot_catalog(
    normalized_rom: &NormalizedRomDigest,
    captures: &[ExecutableImageCapture],
    allowed_entries: &[u32],
    immutable_ranges: &[ImmutableAotRange],
) -> Result<Vec<ExternalAotImage>, ExternalAotCatalogError> {
    if captures.is_empty() {
        return Err(ExternalAotCatalogError::EmptyCatalog);
    }
    let allowed = allowed_entries.iter().copied().collect::<BTreeSet<_>>();
    if allowed.is_empty() {
        return Err(ExternalAotCatalogError::EmptyAllowedEntries);
    }
    if let Some(entry) = allowed.iter().find(|entry| !entry.is_multiple_of(4)) {
        return Err(ExternalAotCatalogError::UnalignedAllowedEntry(*entry));
    }
    let normalized_rom_sha256 = String::from(normalized_rom.clone());
    let mut images = Vec::with_capacity(captures.len());
    let mut identities = BTreeSet::new();
    for capture in captures {
        if capture.normalized_rom_sha256 != *normalized_rom {
            return Err(ExternalAotCatalogError::RomIdentityMismatch {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            });
        }
        let capture_document = serde_json::to_vec(capture).map_err(|_| {
            ExternalAotCatalogError::InvalidImageGeometry {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            }
        })?;
        if parse_executable_image_capture(&capture_document, normalized_rom).is_err() {
            return Err(ExternalAotCatalogError::InvalidImageGeometry {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            });
        }
        let Some(va_end) = capture.va_start.checked_add(capture.byte_len) else {
            return Err(ExternalAotCatalogError::InvalidImageGeometry {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            });
        };
        if capture.byte_len == 0
            || !capture.va_start.is_multiple_of(4)
            || !capture.byte_len.is_multiple_of(4)
            || capture.words.len() != capture.byte_len as usize / 4
            || !(capture.va_start..va_end).contains(&capture.first_executed_pc)
        {
            return Err(ExternalAotCatalogError::InvalidImageGeometry {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            });
        }
        if !identities.insert((capture.image_id.clone(), capture.generation)) {
            return Err(ExternalAotCatalogError::DuplicateImageIdentity {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            });
        }
        let owned_entries = allowed
            .range(capture.va_start..va_end)
            .copied()
            .filter(|entry| entry.checked_add(4).is_some_and(|end| end <= va_end))
            .collect::<Vec<_>>();
        if owned_entries.is_empty() {
            return Err(ExternalAotCatalogError::ImageOwnsNoAllowedEntry {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
            });
        }
        if !owned_entries.contains(&capture.first_executed_pc) {
            return Err(ExternalAotCatalogError::FirstExecutedPcIsNotOwnedEntry {
                image_id: capture.image_id.clone(),
                generation: capture.generation,
                first_executed_pc: capture.first_executed_pc,
            });
        }
        let bank_id = external_aot_bank_id(&normalized_rom_sha256, capture);
        images.push(ExternalAotImage {
            capture: capture.clone(),
            bank_id,
            owned_entries,
        });
    }
    images.sort_by(|left, right| {
        left.capture
            .va_start
            .cmp(&right.capture.va_start)
            .then_with(|| left.capture.image_id.cmp(&right.capture.image_id))
            .then_with(|| left.capture.generation.cmp(&right.capture.generation))
    });
    for pair in images.windows(2) {
        let left_end = pair[0].capture.va_start + pair[0].capture.byte_len;
        if pair[1].capture.va_start < left_end {
            return Err(ExternalAotCatalogError::OverlappingImages {
                first_image_id: pair[0].capture.image_id.clone(),
                first_generation: pair[0].capture.generation,
                second_image_id: pair[1].capture.image_id.clone(),
                second_generation: pair[1].capture.generation,
            });
        }
    }
    let mut bank_ids = immutable_ranges
        .iter()
        .map(|range| range.bank_id)
        .collect::<BTreeSet<_>>();
    if let Some(range) = immutable_ranges.iter().find(|range| {
        range.label.trim().is_empty()
            || range.va_start >= range.va_end
            || !range.va_start.is_multiple_of(4)
            || !range.va_end.is_multiple_of(4)
    }) {
        return Err(ExternalAotCatalogError::InvalidImmutableRange {
            immutable_label: range.label.clone(),
        });
    }
    if bank_ids.len() != immutable_ranges.len() {
        return Err(ExternalAotCatalogError::BankIdCollision {
            bank_id: immutable_ranges
                .iter()
                .map(|range| range.bank_id)
                .find(|bank_id| {
                    immutable_ranges
                        .iter()
                        .filter(|range| range.bank_id == *bank_id)
                        .count()
                        > 1
                })
                .unwrap_or(0),
        });
    }
    for image in &images {
        let image_end = image.capture.va_start + image.capture.byte_len;
        if let Some(range) = immutable_ranges.iter().find(|range| {
            overlaps(
                image.capture.va_start,
                image_end,
                range.va_start,
                range.va_end,
            )
        }) {
            return Err(ExternalAotCatalogError::OverlapsImmutableRange {
                image_id: image.capture.image_id.clone(),
                generation: image.capture.generation,
                immutable_label: range.label.clone(),
            });
        }
        if !bank_ids.insert(image.bank_id) {
            return Err(ExternalAotCatalogError::BankIdCollision {
                bank_id: image.bank_id,
            });
        }
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{ExecutableImageLineage, EXECUTABLE_IMAGE_SCHEMA};

    const ROM_SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn capture(image_id: &str, va_start: u32, words: &[u32]) -> ExecutableImageCapture {
        let content = words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        ExecutableImageCapture {
            schema: EXECUTABLE_IMAGE_SCHEMA.to_string(),
            producer: "external-aot-test".to_string(),
            normalized_rom_sha256: NormalizedRomDigest::try_from(ROM_SHA.to_string()).unwrap(),
            image_id: image_id.to_string(),
            lineage: ExecutableImageLineage::CpuProduced,
            generation: 0,
            capture_pc: va_start,
            first_executed_pc: va_start,
            retired_instructions: 1,
            va_start,
            byte_len: words.len() as u32 * 4,
            sha256: format!("{:x}", Sha256::digest(content)),
            words: words.to_vec(),
        }
    }

    fn rom_digest() -> NormalizedRomDigest {
        NormalizedRomDigest::try_from(ROM_SHA.to_string()).unwrap()
    }

    #[test]
    fn catalog_is_deterministic_and_accepts_disjoint_vector_images() {
        let first = capture("general", 0x8000_0180, &[1, 2, 3, 4]);
        let refill = capture("refill", 0x8000_0000, &[5, 6, 7, 8]);
        let allowed = [0x8000_0000, 0x8000_0180];
        let forward = build_external_aot_catalog(
            &rom_digest(),
            &[first.clone(), refill.clone()],
            &allowed,
            &[],
        )
        .unwrap();
        let reverse =
            build_external_aot_catalog(&rom_digest(), &[refill, first], &allowed, &[]).unwrap();
        assert_eq!(forward, reverse);
        assert_eq!(
            forward
                .iter()
                .map(|image| image.capture.va_start)
                .collect::<Vec<_>>(),
            vec![0x8000_0000, 0x8000_0180]
        );
    }

    #[test]
    fn overlap_unowned_and_immutable_aliases_fail_closed() {
        let allowed = [0x8000_0000, 0x8000_0008];
        let wide = capture("wide", 0x8000_0000, &[1, 2, 3, 4]);
        let overlap = capture("overlap", 0x8000_0008, &[5, 6, 7, 8]);
        assert!(matches!(
            build_external_aot_catalog(&rom_digest(), &[wide.clone(), overlap], &allowed, &[]),
            Err(ExternalAotCatalogError::OverlappingImages { .. })
        ));

        let unrelated = capture("unrelated", 0x8000_1000, &[1]);
        assert!(matches!(
            build_external_aot_catalog(&rom_digest(), &[unrelated], &allowed, &[]),
            Err(ExternalAotCatalogError::ImageOwnsNoAllowedEntry { .. })
        ));

        let immutable = [ImmutableAotRange {
            label: "resident".to_string(),
            bank_id: 7,
            va_start: 0x8000_000c,
            va_end: 0x8000_1000,
        }];
        assert!(matches!(
            build_external_aot_catalog(&rom_digest(), &[wide], &allowed, &immutable),
            Err(ExternalAotCatalogError::OverlapsImmutableRange { .. })
        ));
    }

    #[test]
    fn first_fetch_must_be_one_of_the_owned_entries() {
        let mut image = capture("wide", 0x8000_0000, &[1, 2, 3, 4]);
        image.first_executed_pc = 0x8000_0004;
        assert!(matches!(
            build_external_aot_catalog(&rom_digest(), &[image], &[0x8000_0000, 0x8000_0008], &[]),
            Err(ExternalAotCatalogError::FirstExecutedPcIsNotOwnedEntry { .. })
        ));
    }

    #[test]
    fn six_images_and_one_multi_entry_image_preserve_exact_ownership() {
        let entries = [
            0x8000_0000,
            0x8000_0080,
            0x8000_0180,
            0xbfc0_0200,
            0xbfc0_0280,
            0xbfc0_0380,
        ];
        let captures = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| capture(&format!("vector-{index}"), *entry, &[index as u32]))
            .collect::<Vec<_>>();
        let catalog = build_external_aot_catalog(&rom_digest(), &captures, &entries, &[]).unwrap();
        assert_eq!(catalog.len(), 6);
        assert!(catalog.iter().all(|image| image.owned_entries.len() == 1));

        let spanning_words = (0..=0x80 / 4).collect::<Vec<_>>();
        let spanning = capture("two-vectors", 0x8000_0000, &spanning_words);
        let catalog = build_external_aot_catalog(
            &rom_digest(),
            &[spanning],
            &[0x8000_0000, 0x8000_0080],
            &[],
        )
        .unwrap();
        assert_eq!(catalog[0].owned_entries, vec![0x8000_0000, 0x8000_0080]);
    }

    #[test]
    fn duplicate_identity_and_bank_collision_fail_closed() {
        let image = capture("general", 0x8000_0180, &[1, 2, 3, 4]);
        assert!(matches!(
            build_external_aot_catalog(
                &rom_digest(),
                &[image.clone(), image.clone()],
                &[0x8000_0180],
                &[]
            ),
            Err(ExternalAotCatalogError::DuplicateImageIdentity { .. })
        ));

        let admitted =
            build_external_aot_catalog(&rom_digest(), &[image.clone()], &[0x8000_0180], &[])
                .unwrap();
        let immutable = [ImmutableAotRange {
            label: "nonoverlapping-collision".to_string(),
            bank_id: admitted[0].bank_id,
            va_start: 0x9000_0000,
            va_end: 0x9000_0004,
        }];
        assert_eq!(
            build_external_aot_catalog(&rom_digest(), &[image], &[0x8000_0180], &immutable),
            Err(ExternalAotCatalogError::BankIdCollision {
                bank_id: admitted[0].bank_id,
            })
        );

        let invalid = [ImmutableAotRange {
            label: "invalid".to_string(),
            bank_id: 9,
            va_start: 0x9000_0004,
            va_end: 0x9000_0000,
        }];
        assert_eq!(
            build_external_aot_catalog(
                &rom_digest(),
                &[capture("fresh", 0x8000_0180, &[1])],
                &[0x8000_0180],
                &invalid,
            ),
            Err(ExternalAotCatalogError::InvalidImmutableRange {
                immutable_label: "invalid".to_string(),
            })
        );
    }
}
