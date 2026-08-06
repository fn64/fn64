//! Content-addressed dense AOT manifest construction.
//!
//! Unlike `BlockPackV1`, this pack does not encode a discovered CFG subset.
//! Every aligned word of every supplied immutable materialization is owned by
//! exactly one compile shard. ROM bytes remain outside the manifest; source
//! intervals and digests let a user's local build reproduce and verify them.

use crate::overlay_recipe::OverlayLoadRecipeV1;
use crate::rom::NormalizedRom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DENSE_AOT_PACK_SCHEMA_V1: &str = "fn64.dense-aot-pack.v1";
pub const DENSE_AOT_SHARD_BYTES: u32 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenseAotPackV1 {
    pub schema: String,
    pub normalized_rom_sha256: String,
    pub generations: Vec<DenseAotGenerationV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenseAotGenerationV1 {
    pub name: String,
    pub bank_id: u64,
    pub source_rom_start: u32,
    pub source_rom_end: u32,
    pub load_start: u32,
    pub load_end: u32,
    pub text_start: u32,
    pub text_end: u32,
    pub data_start: u32,
    pub data_end: u32,
    pub bss_start: u32,
    pub bss_end: u32,
    pub loaded_sha256: String,
    pub aligned_entry_count: u32,
    pub shards: Vec<DenseAotShardV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenseAotShardV1 {
    pub index: u32,
    pub source_rom_start: u32,
    pub source_rom_end: u32,
    pub va_start: u32,
    pub va_end: u32,
    pub sha256: String,
    pub delay_lookahead: Option<u32>,
    pub artifact_identity: String,
}

#[derive(Clone, Copy, Debug)]
pub struct DenseAotGenerationInput<'a> {
    pub name: &'a str,
    pub source_rom_start: u32,
    pub source_rom_end: u32,
    pub load_start: u32,
    pub text_start: u32,
    pub text_end: u32,
    pub data_start: u32,
    pub data_end: u32,
    pub bss_start: u32,
    pub bss_end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DenseAotPackError {
    EmptyName,
    UnalignedGeometry,
    InvalidRangeRelations,
    SourceOutsideRom,
    AddressOverflow,
    EntryCountOverflow,
}

impl<'a> From<(&'a str, &'a OverlayLoadRecipeV1)> for DenseAotGenerationInput<'a> {
    fn from((name, recipe): (&'a str, &'a OverlayLoadRecipeV1)) -> Self {
        Self {
            name,
            source_rom_start: recipe.rom_start,
            // TEXT extent, not the whole loaded image. See
            // `overlay_recipe::generation_source_span` -- every consumer
            // derives from that one function so their shard extents, which
            // are folded into catalog digests, cannot disagree.
            source_rom_end: recipe.rom_start
                + crate::overlay_recipe::generation_source_span(recipe),
            load_start: recipe.load_start,
            text_start: recipe.text_start,
            text_end: recipe.text_end,
            data_start: recipe.data_start,
            data_end: recipe.data_end,
            bss_start: recipe.bss_start,
            bss_end: recipe.bss_end,
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Stable identity used by one generated dense shard runner.
///
/// This is intentionally distinct from a generation's manifest `bank_id`:
/// the runtime registers independently compiled 64 KiB artifacts, while the
/// manifest groups their complete union under one immutable generation.
pub fn dense_aot_artifact_bank_id(
    normalized_rom_sha256: &str,
    generation: &str,
    va_start: u32,
    words: &[u32],
) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:dense-aot-artifact:v1:");
    hasher.update(normalized_rom_sha256.as_bytes());
    hasher.update(generation.as_bytes());
    hasher.update(va_start.to_be_bytes());
    for word in words {
        hasher.update(word.to_be_bytes());
    }
    u64::from_be_bytes(hasher.finalize()[..8].try_into().unwrap())
}

/// Full source identity of one immutable dense shard.
///
/// Unlike the runtime [`dense_aot_artifact_bank_id`], this retains all 256
/// digest bits and binds both ROM/load geometry and the exact source bytes.
/// The manifest producer and independently compiled shard crates call this
/// same function before their artifacts meet at the production admission
/// boundary.
pub fn dense_aot_shard_source_identity(
    normalized_rom_sha256: &str,
    generation: &str,
    source_rom_start: u32,
    source_rom_end: u32,
    va_start: u32,
    va_end: u32,
    shard_bytes: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:dense-aot-shard-source:v1:");
    hasher.update(normalized_rom_sha256.as_bytes());
    hasher.update((generation.len() as u64).to_be_bytes());
    hasher.update(generation.as_bytes());
    for value in [source_rom_start, source_rom_end, va_start, va_end] {
        hasher.update(value.to_be_bytes());
    }
    hasher.update((shard_bytes.len() as u64).to_be_bytes());
    hasher.update(shard_bytes);
    hasher.finalize().into()
}

fn generation_bank_id(rom_sha256: &str, input: DenseAotGenerationInput<'_>, digest: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:dense-aot-generation:v1:");
    hasher.update(rom_sha256.as_bytes());
    hasher.update(input.name.as_bytes());
    for value in [
        input.source_rom_start,
        input.source_rom_end,
        input.load_start,
        input.text_start,
        input.text_end,
        input.data_start,
        input.data_end,
        input.bss_start,
        input.bss_end,
    ] {
        hasher.update(value.to_be_bytes());
    }
    hasher.update(digest.as_bytes());
    u64::from_be_bytes(hasher.finalize()[..8].try_into().unwrap())
}

pub fn build_dense_aot_pack_v1(
    rom: &NormalizedRom,
    inputs: &[DenseAotGenerationInput<'_>],
) -> Result<DenseAotPackV1, DenseAotPackError> {
    let mut generations = Vec::with_capacity(inputs.len());
    for &input in inputs {
        if input.name.trim().is_empty() {
            return Err(DenseAotPackError::EmptyName);
        }
        let values = [
            input.source_rom_start,
            input.source_rom_end,
            input.load_start,
            input.text_start,
            input.text_end,
            input.data_start,
            input.data_end,
            input.bss_start,
            input.bss_end,
        ];
        if values.iter().any(|value| !value.is_multiple_of(4)) {
            return Err(DenseAotPackError::UnalignedGeometry);
        }
        let source_len = input
            .source_rom_end
            .checked_sub(input.source_rom_start)
            .ok_or(DenseAotPackError::InvalidRangeRelations)?;
        let load_end = input
            .load_start
            .checked_add(source_len)
            .ok_or(DenseAotPackError::AddressOverflow)?;
        if source_len == 0
            || input.load_start != input.text_start
            || input.text_start >= input.text_end
            || input.text_end != input.data_start
            || input.data_start > input.data_end
            || input.data_end != input.bss_start
            || input.bss_start > input.bss_end
            // The dense source may cover only the TEXT extent rather than the
            // whole loaded image. The data section is mutable -- a correct
            // program writes it at runtime -- so a generation digested over it
            // cannot survive execution. Require the source to cover at least
            // the text and never exceed the loaded image, instead of demanding
            // it equal `data_end` exactly.
            || load_end < input.text_end
            || load_end > input.data_end
        {
            return Err(DenseAotPackError::InvalidRangeRelations);
        }
        let bytes = rom
            .bytes
            .get(input.source_rom_start as usize..input.source_rom_end as usize)
            .ok_or(DenseAotPackError::SourceOutsideRom)?;
        let loaded_sha256 = sha256(bytes);
        let bank_id = generation_bank_id(&rom.sha256, input, &loaded_sha256);
        let aligned_entry_count = source_len / 4;
        let mut shards = Vec::new();
        let mut offset = 0u32;
        while offset < source_len {
            let shard_len = DENSE_AOT_SHARD_BYTES.min(source_len - offset);
            let source_start = input.source_rom_start + offset;
            let source_end = source_start + shard_len;
            let va_start = input.load_start + offset;
            let va_end = va_start + shard_len;
            let shard_bytes = &rom.bytes[source_start as usize..source_end as usize];
            let delay_lookahead = rom
                .bytes
                .get(source_end as usize..source_end as usize + 4)
                .filter(|_| source_end < input.source_rom_end)
                .map(|word| u32::from_be_bytes(word.try_into().unwrap()));
            let shard_sha256 = sha256(shard_bytes);
            let identity = dense_aot_shard_source_identity(
                &rom.sha256,
                input.name,
                source_start,
                source_end,
                va_start,
                va_end,
                shard_bytes,
            );
            shards.push(DenseAotShardV1 {
                index: shards.len() as u32,
                source_rom_start: source_start,
                source_rom_end: source_end,
                va_start,
                va_end,
                sha256: shard_sha256,
                delay_lookahead,
                artifact_identity: identity.iter().map(|byte| format!("{byte:02x}")).collect(),
            });
            offset = offset
                .checked_add(shard_len)
                .ok_or(DenseAotPackError::AddressOverflow)?;
        }
        if shards
            .iter()
            .map(|shard| (shard.va_end - shard.va_start) / 4)
            .sum::<u32>()
            != aligned_entry_count
        {
            return Err(DenseAotPackError::EntryCountOverflow);
        }
        generations.push(DenseAotGenerationV1 {
            name: input.name.to_string(),
            bank_id,
            source_rom_start: input.source_rom_start,
            source_rom_end: input.source_rom_end,
            load_start: input.load_start,
            load_end,
            text_start: input.text_start,
            text_end: input.text_end,
            data_start: input.data_start,
            data_end: input.data_end,
            bss_start: input.bss_start,
            bss_end: input.bss_end,
            loaded_sha256,
            aligned_entry_count,
            shards,
        });
    }
    Ok(DenseAotPackV1 {
        schema: DENSE_AOT_PACK_SCHEMA_V1.to_string(),
        normalized_rom_sha256: rom.sha256.clone(),
        generations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_union_owns_every_aligned_entry_once() {
        let mut bytes = vec![0u8; 0x2_0000];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        bytes[0x20..0x24].copy_from_slice(b"TEST");
        let rom = crate::normalize(&bytes).unwrap();
        let pack = build_dense_aot_pack_v1(
            &rom,
            &[DenseAotGenerationInput {
                name: "generation",
                source_rom_start: 0x1000,
                source_rom_end: 0x1_1040,
                load_start: 0x8000_1000,
                text_start: 0x8000_1000,
                text_end: 0x8000_9000,
                data_start: 0x8000_9000,
                data_end: 0x8001_1040,
                bss_start: 0x8001_1040,
                bss_end: 0x8001_2000,
            }],
        )
        .unwrap();
        let generation = &pack.generations[0];
        assert_eq!(generation.shards.len(), 2);
        assert_eq!(generation.shards[0].va_end, generation.shards[1].va_start);
        assert_eq!(generation.aligned_entry_count, 0x1_0040 / 4);
        assert_eq!(generation.shards[0].delay_lookahead, Some(0));
        assert_eq!(generation.shards[1].delay_lookahead, None);
    }

    #[test]
    fn dense_artifact_identity_is_bound_to_every_input() {
        let words = [0x3c01_8000, 0x3421_0400];
        let baseline = dense_aot_artifact_bank_id(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "boot",
            0x8000_0400,
            &words,
        );
        assert_ne!(
            baseline,
            dense_aot_artifact_bank_id(
                "1123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x8000_0400,
                &words,
            )
        );
        assert_ne!(
            baseline,
            dense_aot_artifact_bank_id(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "overlay",
                0x8000_0400,
                &words,
            )
        );
        assert_ne!(
            baseline,
            dense_aot_artifact_bank_id(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x8001_0400,
                &words,
            )
        );
        assert_ne!(
            baseline,
            dense_aot_artifact_bank_id(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x8000_0400,
                &[words[0], words[1] ^ 1],
            )
        );
    }

    #[test]
    fn shard_source_identity_is_bound_to_every_component() {
        let bytes = [1, 2, 3, 4];
        let baseline = dense_aot_shard_source_identity(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "boot",
            0x1000,
            0x1004,
            0x8000_0400,
            0x8000_0404,
            &bytes,
        );
        let variants = [
            dense_aot_shard_source_identity(
                "1123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x1000,
                0x1004,
                0x8000_0400,
                0x8000_0404,
                &bytes,
            ),
            dense_aot_shard_source_identity(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "overlay",
                0x1000,
                0x1004,
                0x8000_0400,
                0x8000_0404,
                &bytes,
            ),
            dense_aot_shard_source_identity(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x2000,
                0x2004,
                0x8000_0400,
                0x8000_0404,
                &bytes,
            ),
            dense_aot_shard_source_identity(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x1000,
                0x1004,
                0x8001_0400,
                0x8001_0404,
                &bytes,
            ),
            dense_aot_shard_source_identity(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "boot",
                0x1000,
                0x1004,
                0x8000_0400,
                0x8000_0404,
                &[1, 2, 3, 5],
            ),
        ];
        assert!(variants.into_iter().all(|identity| identity != baseline));
    }
}
