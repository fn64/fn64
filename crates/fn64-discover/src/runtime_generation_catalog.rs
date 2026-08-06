//! Runtime generation-catalog construction from validated ROM-bound geometry.
//!
//! The generated harness and offline gates call this same constructor so
//! shard identities, image digests, and physical backing rules cannot drift.

use crate::dense_aot_pack::{dense_aot_artifact_bank_id, DenseAotPackV1, DENSE_AOT_SHARD_BYTES};
use crate::generation_topology::{CatalogGenerationRoleV1, GenerationTopologyV1};
use crate::NormalizedRom;
use fn64_recomp_rs::{
    BackedExecutableSpanV1, BackedPrecompiledGenerationCatalogV1, BankId, GenerationId, GuestPc,
    PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledGenerationCatalog,
    PrecompiledShard,
};
use sha2::{Digest, Sha256};

pub const RESIDENT_TAIL_ARTIFACT_IDENTITY_V1: &str = "resident_tail";

/// Build the exact dense-only runtime catalog represented by a validated
/// generation topology.
///
/// External/runtime-captured executable images are intentionally absent. They
/// augment this immutable ROM-derived denominator through a separate path.
pub fn build_backed_dense_generation_catalog_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
) -> Result<BackedPrecompiledGenerationCatalogV1, String> {
    let resident = dense_pack.generations.first().ok_or_else(|| {
        "runtime generation catalog lacks a resident dense generation".to_string()
    })?;
    let mut catalog = PrecompiledGenerationCatalog::new();
    let mut backings = Vec::with_capacity(topology.generations.len());

    for generation in &topology.generations {
        let dense = match generation.role {
            CatalogGenerationRoleV1::ResidentTail => resident,
            CatalogGenerationRoleV1::Overlay => {
                let mut matches = dense_pack
                    .generations
                    .iter()
                    .filter(|candidate| candidate.name == generation.materialized_bank);
                let dense = matches.next().ok_or_else(|| {
                    format!(
                        "runtime generation {} has no matching dense bank",
                        generation.name
                    )
                })?;
                if matches.next().is_some() {
                    return Err(format!(
                        "runtime generation {} has multiple matching dense banks",
                        generation.name
                    ));
                }
                dense
            }
        };
        let source_offset = generation
            .image_start
            .checked_sub(dense.load_start)
            .ok_or_else(|| {
                format!(
                    "runtime generation {} begins before dense bank {}",
                    generation.name, dense.name
                )
            })?;
        let source_start = dense
            .source_rom_start
            .checked_add(source_offset)
            .ok_or_else(|| {
                format!(
                    "runtime generation {} source start overflow",
                    generation.name
                )
            })?;
        let byte_len = generation
            .image_end
            .checked_sub(generation.image_start)
            .ok_or_else(|| {
                format!(
                    "runtime generation {} has inverted image geometry",
                    generation.name
                )
            })?;
        let source_end = source_start
            .checked_add(byte_len)
            .ok_or_else(|| format!("runtime generation {} source end overflow", generation.name))?;
        if source_end > dense.source_rom_end {
            return Err(format!(
                "runtime generation {} exceeds dense source bank {}",
                generation.name, dense.name
            ));
        }
        let bytes = rom
            .bytes
            .get(source_start as usize..source_end as usize)
            .ok_or_else(|| {
                format!(
                    "runtime generation {} source is outside the ROM",
                    generation.name
                )
            })?;
        let image_sha256: [u8; 32] = Sha256::digest(bytes).into();
        let image_sha256_hex = image_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if image_sha256_hex != generation.image_sha256 {
            return Err(format!(
                "runtime generation {} ROM digest disagrees with topology",
                generation.name
            ));
        }
        let artifact_identity_name = match generation.role {
            CatalogGenerationRoleV1::ResidentTail => RESIDENT_TAIL_ARTIFACT_IDENTITY_V1,
            CatalogGenerationRoleV1::Overlay => generation.materialized_bank.as_str(),
        };
        // Shards tile in whole DENSE_AOT_SHARD_BYTES blocks and the LAST one
        // may overhang `image_end` -- a generation may end mid-shard. So the
        // shard list is chunked from the dense bank's own bytes, not from the
        // digest slice above: chunking `bytes` would emit a final shard that
        // stops at `image_end` and disagree with the shard geometry the pack
        // emits, which is exactly the catalog-digest mismatch this produced.
        let shard_span_end = source_start
            .checked_add(byte_len.div_ceil(DENSE_AOT_SHARD_BYTES) * DENSE_AOT_SHARD_BYTES)
            .ok_or_else(|| format!("runtime generation {} shard span overflow", generation.name))?
            .min(dense.source_rom_end);
        let shard_bytes_all = rom
            .bytes
            .get(source_start as usize..shard_span_end as usize)
            .ok_or_else(|| {
                format!(
                    "runtime generation {} shard span is outside the ROM",
                    generation.name
                )
            })?;
        let shards = shard_bytes_all
            .chunks(DENSE_AOT_SHARD_BYTES as usize)
            .enumerate()
            .map(|(index, shard_bytes)| {
                if !shard_bytes.len().is_multiple_of(4) {
                    return Err(format!(
                        "runtime generation {} shard {index} is not instruction-aligned",
                        generation.name
                    ));
                }
                let offset =
                    u32::try_from(index * DENSE_AOT_SHARD_BYTES as usize).map_err(|_| {
                        format!(
                            "runtime generation {} shard offset overflow",
                            generation.name
                        )
                    })?;
                let start = generation.image_start.checked_add(offset).ok_or_else(|| {
                    format!("runtime generation {} shard VA overflow", generation.name)
                })?;
                let end = start
                    .checked_add(u32::try_from(shard_bytes.len()).map_err(|_| {
                        format!(
                            "runtime generation {} shard length overflow",
                            generation.name
                        )
                    })?)
                    .ok_or_else(|| {
                        format!("runtime generation {} shard end overflow", generation.name)
                    })?;
                let words = shard_bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                    .collect::<Vec<_>>();
                let bank =
                    dense_aot_artifact_bank_id(&rom.sha256, artifact_identity_name, start, &words);
                PrecompiledShard::new(BankId::new(bank), GuestPc::new(start), GuestPc::new(end))
                    .map_err(|error| {
                        format!(
                            "building runtime generation {} shard {index}: {error}",
                            generation.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let generation_id = GenerationId::new(generation.generation_id);
        let image_start = GuestPc::new(generation.image_start);
        let image_end = GuestPc::new(generation.image_end);
        let invalidation_start = GuestPc::new(generation.invalidation_start);
        let invalidation_end = GuestPc::new(generation.invalidation_end);
        catalog
            .register(
                PrecompiledGeneration::new(
                    generation_id,
                    image_start,
                    image_end,
                    invalidation_start,
                    invalidation_end,
                    image_sha256,
                    shards,
                )
                .map_err(|error| {
                    format!("building runtime generation {}: {error}", generation.name)
                })?,
            )
            .map_err(|error| {
                format!(
                    "registering runtime generation {}: {error}",
                    generation.name
                )
            })?;
        if !(0x8000_0000..0xc000_0000).contains(&generation.invalidation_start)
            || generation.invalidation_end > 0xc000_0000
        {
            return Err(format!(
                "runtime generation {} invalidation range is not direct-mapped KSEG",
                generation.name
            ));
        }
        let backing_len = generation
            .invalidation_end
            .checked_sub(generation.invalidation_start)
            .ok_or_else(|| {
                format!(
                    "runtime generation {} has inverted invalidation geometry",
                    generation.name
                )
            })?;
        let span = BackedExecutableSpanV1::new(
            invalidation_start,
            generation.invalidation_start & 0x1fff_ffff,
            backing_len,
        )
        .map_err(|error| {
            format!(
                "building runtime generation {} physical backing: {error}",
                generation.name
            )
        })?;
        backings.push(
            PrecompiledGenerationBackingV1::new(generation_id, vec![span]).map_err(|error| {
                format!(
                    "closing runtime generation {} physical backing: {error}",
                    generation.name
                )
            })?,
        );
    }

    BackedPrecompiledGenerationCatalogV1::new(catalog, backings)
        .map_err(|error| format!("closing backed runtime generation catalog: {error}"))
}

#[cfg(test)]
mod tests {
    use crate::generation_topology::resident_tail_generation_id_v1;

    #[test]
    fn resident_tail_generation_identity_v1_is_compatible() {
        let actual = resident_tail_generation_id_v1(
            b"fn64:test-resident-tail:v1:",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            0x800e_0000,
            0x8010_0400,
            0x800e_0000,
            0x8014_0000,
            [0x5a; 32],
        );
        assert_eq!(actual, 0x686e_adec_80ab_7bc5);
    }
}
