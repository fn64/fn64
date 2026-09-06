//! Runtime generation-catalog construction from validated ROM-bound geometry.
//!
//! The generated harness and offline gates call this same constructor so
//! shard identities, image digests, and physical backing rules cannot drift.

use crate::dense_aot_pack::{dense_aot_artifact_bank_id, DenseAotPackV1, DENSE_AOT_SHARD_BYTES};
use crate::generation_topology::{CatalogGenerationRoleV1, GenerationTopologyV1};
use crate::NormalizedRom;
use fn64_cpu_runtime::{
    BackedExecutableSpanV1, BackedPrecompiledGenerationCatalogV1, BankId, GenerationId, GuestPc,
    PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledGenerationCatalog,
    PrecompiledShard,
};
use sha2::{Digest, Sha256};

pub const RESIDENT_TAIL_ARTIFACT_IDENTITY_V1: &str = "resident_tail";

/// Errors building the runtime dense-only generation catalog from a
/// validated generation topology.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeGenerationCatalogError {
    #[error("runtime generation catalog lacks a resident dense generation")]
    NoResidentDenseGeneration,
    #[error("runtime generation {generation} has no matching dense bank")]
    NoMatchingDenseBank { generation: String },
    #[error("runtime generation {generation} has multiple matching dense banks")]
    MultipleMatchingDenseBanks { generation: String },
    #[error("runtime generation {generation} begins before dense bank {dense}")]
    BeginsBeforeDenseBank { generation: String, dense: String },
    #[error("runtime generation {generation} source start overflow")]
    SourceStartOverflow { generation: String },
    #[error("runtime generation {generation} has inverted image geometry")]
    InvertedImageGeometry { generation: String },
    #[error("runtime generation {generation} source end overflow")]
    SourceEndOverflow { generation: String },
    #[error("runtime generation {generation} exceeds dense source bank {dense}")]
    ExceedsDenseSourceBank { generation: String, dense: String },
    #[error("runtime generation {generation} source is outside the ROM")]
    SourceOutsideRom { generation: String },
    #[error("runtime generation {generation} ROM digest disagrees with topology")]
    RomDigestDisagreesWithTopology { generation: String },
    #[error("runtime generation {generation} shard span overflow")]
    ShardSpanOverflow { generation: String },
    #[error("runtime generation {generation} shard span is outside the ROM")]
    ShardSpanOutsideRom { generation: String },
    #[error("runtime generation {generation} shard {index} is not instruction-aligned")]
    ShardNotInstructionAligned { generation: String, index: usize },
    #[error("runtime generation {generation} shard offset overflow")]
    ShardOffsetOverflow { generation: String },
    #[error("runtime generation {generation} shard VA overflow")]
    ShardVaOverflow { generation: String },
    #[error("runtime generation {generation} shard length overflow")]
    ShardLengthOverflow { generation: String },
    #[error("runtime generation {generation} shard end overflow")]
    ShardEndOverflow { generation: String },
    #[error("building runtime generation {generation} shard {index}: {error}")]
    BuildingShard {
        generation: String,
        index: usize,
        error: fn64_cpu_runtime::GenerationCatalogError,
    },
    #[error("building runtime generation {generation}: {error}")]
    BuildingGeneration {
        generation: String,
        error: fn64_cpu_runtime::GenerationCatalogError,
    },
    #[error("registering runtime generation {generation}: {error}")]
    RegisteringGeneration {
        generation: String,
        error: fn64_cpu_runtime::GenerationCatalogError,
    },
    #[error("runtime generation {generation} invalidation range is not direct-mapped KSEG")]
    InvalidationRangeNotKseg { generation: String },
    #[error("runtime generation {generation} has inverted invalidation geometry")]
    InvertedInvalidationGeometry { generation: String },
    #[error("building runtime generation {generation} physical backing: {error}")]
    BuildingPhysicalBacking {
        generation: String,
        error: fn64_cpu_runtime::BackedGenerationCatalogErrorV1,
    },
    #[error("closing runtime generation {generation} physical backing: {error}")]
    ClosingPhysicalBacking {
        generation: String,
        error: fn64_cpu_runtime::BackedGenerationCatalogErrorV1,
    },
    #[error("closing backed runtime generation catalog: {0}")]
    ClosingCatalog(fn64_cpu_runtime::BackedGenerationCatalogErrorV1),
}

/// Build the exact dense-only runtime catalog represented by a validated
/// generation topology.
///
/// External/runtime-captured executable images are intentionally absent. They
/// augment this immutable ROM-derived denominator through a separate path.
pub fn build_backed_dense_generation_catalog_v1(
    rom: &NormalizedRom,
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
) -> Result<BackedPrecompiledGenerationCatalogV1, RuntimeGenerationCatalogError> {
    let resident = dense_pack
        .generations
        .first()
        .ok_or(RuntimeGenerationCatalogError::NoResidentDenseGeneration)?;
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
                    RuntimeGenerationCatalogError::NoMatchingDenseBank {
                        generation: generation.name.clone(),
                    }
                })?;
                if matches.next().is_some() {
                    return Err(RuntimeGenerationCatalogError::MultipleMatchingDenseBanks {
                        generation: generation.name.clone(),
                    });
                }
                dense
            }
        };
        let source_offset = generation
            .image_start
            .checked_sub(dense.load_start)
            .ok_or_else(|| RuntimeGenerationCatalogError::BeginsBeforeDenseBank {
                generation: generation.name.clone(),
                dense: dense.name.clone(),
            })?;
        let source_start = dense
            .source_rom_start
            .checked_add(source_offset)
            .ok_or_else(|| RuntimeGenerationCatalogError::SourceStartOverflow {
                generation: generation.name.clone(),
            })?;
        let byte_len = generation
            .image_end
            .checked_sub(generation.image_start)
            .ok_or_else(|| RuntimeGenerationCatalogError::InvertedImageGeometry {
                generation: generation.name.clone(),
            })?;
        let source_end = source_start.checked_add(byte_len).ok_or_else(|| {
            RuntimeGenerationCatalogError::SourceEndOverflow {
                generation: generation.name.clone(),
            }
        })?;
        if source_end > dense.source_rom_end {
            return Err(RuntimeGenerationCatalogError::ExceedsDenseSourceBank {
                generation: generation.name.clone(),
                dense: dense.name.clone(),
            });
        }
        let bytes = rom
            .bytes
            .get(source_start as usize..source_end as usize)
            .ok_or_else(|| RuntimeGenerationCatalogError::SourceOutsideRom {
                generation: generation.name.clone(),
            })?;
        let image_sha256: [u8; 32] = Sha256::digest(bytes).into();
        let image_sha256_hex = image_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if image_sha256_hex != generation.image_sha256 {
            return Err(RuntimeGenerationCatalogError::RomDigestDisagreesWithTopology {
                generation: generation.name.clone(),
            });
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
            .ok_or_else(|| RuntimeGenerationCatalogError::ShardSpanOverflow {
                generation: generation.name.clone(),
            })?
            .min(dense.source_rom_end);
        let shard_bytes_all = rom
            .bytes
            .get(source_start as usize..shard_span_end as usize)
            .ok_or_else(|| RuntimeGenerationCatalogError::ShardSpanOutsideRom {
                generation: generation.name.clone(),
            })?;
        let shards = shard_bytes_all
            .chunks(DENSE_AOT_SHARD_BYTES as usize)
            .enumerate()
            .map(|(index, shard_bytes)| {
                if !shard_bytes.len().is_multiple_of(4) {
                    return Err(RuntimeGenerationCatalogError::ShardNotInstructionAligned {
                        generation: generation.name.clone(),
                        index,
                    });
                }
                let offset = u32::try_from(index * DENSE_AOT_SHARD_BYTES as usize).map_err(
                    |_| RuntimeGenerationCatalogError::ShardOffsetOverflow {
                        generation: generation.name.clone(),
                    },
                )?;
                let start = generation.image_start.checked_add(offset).ok_or_else(|| {
                    RuntimeGenerationCatalogError::ShardVaOverflow {
                        generation: generation.name.clone(),
                    }
                })?;
                let end = start
                    .checked_add(u32::try_from(shard_bytes.len()).map_err(|_| {
                        RuntimeGenerationCatalogError::ShardLengthOverflow {
                            generation: generation.name.clone(),
                        }
                    })?)
                    .ok_or_else(|| RuntimeGenerationCatalogError::ShardEndOverflow {
                        generation: generation.name.clone(),
                    })?;
                let words = shard_bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                    .collect::<Vec<_>>();
                let bank =
                    dense_aot_artifact_bank_id(&rom.sha256, artifact_identity_name, start, &words);
                PrecompiledShard::new(BankId::new(bank), GuestPc::new(start), GuestPc::new(end))
                    .map_err(|error| RuntimeGenerationCatalogError::BuildingShard {
                        generation: generation.name.clone(),
                        index,
                        error,
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
                .map_err(|error| RuntimeGenerationCatalogError::BuildingGeneration {
                    generation: generation.name.clone(),
                    error,
                })?,
            )
            .map_err(|error| RuntimeGenerationCatalogError::RegisteringGeneration {
                generation: generation.name.clone(),
                error,
            })?;
        if !(0x8000_0000..0xc000_0000).contains(&generation.invalidation_start)
            || generation.invalidation_end > 0xc000_0000
        {
            return Err(RuntimeGenerationCatalogError::InvalidationRangeNotKseg {
                generation: generation.name.clone(),
            });
        }
        let backing_len = generation
            .invalidation_end
            .checked_sub(generation.invalidation_start)
            .ok_or_else(|| RuntimeGenerationCatalogError::InvertedInvalidationGeometry {
                generation: generation.name.clone(),
            })?;
        let span = BackedExecutableSpanV1::new(
            invalidation_start,
            generation.invalidation_start & 0x1fff_ffff,
            backing_len,
        )
        .map_err(|error| RuntimeGenerationCatalogError::BuildingPhysicalBacking {
            generation: generation.name.clone(),
            error,
        })?;
        backings.push(
            PrecompiledGenerationBackingV1::new(generation_id, vec![span]).map_err(|error| {
                RuntimeGenerationCatalogError::ClosingPhysicalBacking {
                    generation: generation.name.clone(),
                    error,
                }
            })?,
        );
    }

    BackedPrecompiledGenerationCatalogV1::new(catalog, backings)
        .map_err(RuntimeGenerationCatalogError::ClosingCatalog)
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
