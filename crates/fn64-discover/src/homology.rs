//! Conservative cross-ROM function-homology candidates.
//!
//! This module deliberately stops short of proof. Relocation masking throws
//! information away, so even a unique whole-body match is only a candidate
//! for the ordinary fact database to corroborate with control-flow, load-image,
//! and (where available) dynamic evidence.
//!
//! The input functions are prior, independently-derived boundaries. The index
//! never invents a source boundary and never lets a match cross a target code
//! region. Its fixed-width n-gram index is an accelerator only: every emitted
//! candidate passes collision-safe, full-body validation against normalized
//! words.

use std::collections::{BTreeMap, BTreeSet};

/// One independently-derived function body from a reference ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownFunction {
    pub identity: String,
    pub bank: String,
    pub va_start: u32,
    pub words: Vec<u32>,
}

/// A target interval already classified as potentially executable.
///
/// Multiple non-overlapping regions may have the same bank identity. Keeping
/// the region boundary explicit prevents a fingerprint from spanning a data
/// gap or concatenating the end of one load image with the start of another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRegion {
    pub bank: String,
    pub va_start: u32,
    pub words: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CodeLocation {
    pub bank: String,
    pub va: u32,
}

/// A homology proposal. This type intentionally has no proof-state field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomologyCandidate {
    pub source_identity: String,
    pub source: CodeLocation,
    pub target: CodeLocation,
    pub word_len: usize,
    /// Number of non-repetitive source n-grams that voted for this location.
    /// This is diagnostic evidence, not a confidence score.
    pub anchor_votes: usize,
    pub total_anchors: usize,
}

/// Every query has an explicit result, including unresolved cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomologyResult {
    Candidate(HomologyCandidate),
    Ambiguous {
        source_identity: String,
        locations: Vec<CodeLocation>,
    },
    Unmatched {
        source_identity: String,
        best_anchor_votes: usize,
        total_anchors: usize,
    },
}

/// Hard resource bounds and selectivity controls for index construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomologyConfig {
    pub ngram_words: usize,
    pub min_function_words: usize,
    pub max_function_words: usize,
    pub max_source_functions: usize,
    pub max_target_regions: usize,
    pub max_target_words: usize,
    /// Hash buckets above this size are too repetitive to be useful anchors.
    pub max_postings_per_anchor: usize,
    pub min_anchor_votes: usize,
}

impl Default for HomologyConfig {
    fn default() -> Self {
        Self {
            ngram_words: 4,
            min_function_words: 8,
            max_function_words: 16 * 1024,
            max_source_functions: 100_000,
            max_target_regions: 100_000,
            max_target_words: 32 * 1024 * 1024,
            max_postings_per_anchor: 256,
            min_anchor_votes: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomologyError {
    InvalidConfig(&'static str),
    TooManySources {
        count: usize,
        limit: usize,
    },
    TooManyRegions {
        count: usize,
        limit: usize,
    },
    TooManyTargetWords {
        count: usize,
        limit: usize,
    },
    EmptyIdentity,
    DuplicateIdentity(String),
    EmptyBank,
    EmptyRegion {
        bank: String,
        va_start: u32,
    },
    UnalignedAddress {
        bank: String,
        va: u32,
    },
    AddressOverflow {
        bank: String,
        va_start: u32,
        words: usize,
    },
    OverlappingRegions {
        bank: String,
        va: u32,
    },
    FunctionTooShort {
        identity: String,
        words: usize,
        min: usize,
    },
    FunctionTooLong {
        identity: String,
        words: usize,
        max: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AnchorKey {
    first: u64,
    second: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Occurrence {
    region: usize,
    word_offset: usize,
}

#[derive(Debug, Clone)]
struct IndexedRegion {
    input: CodeRegion,
    normalized: Vec<u32>,
}

/// Reusable deterministic target index.
///
/// `BTreeMap` keeps serialization/debugging order stable. The fixed-size key
/// avoids storing every n-gram as an allocated vector; hash equality is never
/// trusted, because query and whole-body comparisons use the actual words.
#[derive(Debug, Clone)]
pub struct HomologyIndex {
    config: HomologyConfig,
    regions: Vec<IndexedRegion>,
    postings: BTreeMap<AnchorKey, Vec<Occurrence>>,
}

impl HomologyIndex {
    pub fn build(regions: &[CodeRegion], config: HomologyConfig) -> Result<Self, HomologyError> {
        validate_config(config)?;
        if regions.len() > config.max_target_regions {
            return Err(HomologyError::TooManyRegions {
                count: regions.len(),
                limit: config.max_target_regions,
            });
        }

        let target_words = regions.iter().try_fold(0usize, |total, region| {
            total
                .checked_add(region.words.len())
                .ok_or(HomologyError::TooManyTargetWords {
                    count: usize::MAX,
                    limit: config.max_target_words,
                })
        })?;
        if target_words > config.max_target_words {
            return Err(HomologyError::TooManyTargetWords {
                count: target_words,
                limit: config.max_target_words,
            });
        }

        let mut canonical = regions.to_vec();
        canonical.sort_by(|a, b| {
            (&a.bank, a.va_start, a.words.len()).cmp(&(&b.bank, b.va_start, b.words.len()))
        });
        validate_regions(&canonical)?;

        let indexed: Vec<_> = canonical
            .into_iter()
            .map(|input| IndexedRegion {
                normalized: normalize_body(&input.words),
                input,
            })
            .collect();

        let mut postings: BTreeMap<AnchorKey, Vec<Occurrence>> = BTreeMap::new();
        for (region_index, region) in indexed.iter().enumerate() {
            if region.normalized.len() < config.ngram_words {
                continue;
            }
            for word_offset in 0..=region.normalized.len() - config.ngram_words {
                let words = &region.normalized[word_offset..word_offset + config.ngram_words];
                postings
                    .entry(anchor_key(words))
                    .or_default()
                    .push(Occurrence {
                        region: region_index,
                        word_offset,
                    });
            }
        }

        Ok(Self {
            config,
            regions: indexed,
            postings,
        })
    }

    pub fn query(&self, sources: &[KnownFunction]) -> Result<Vec<HomologyResult>, HomologyError> {
        if sources.len() > self.config.max_source_functions {
            return Err(HomologyError::TooManySources {
                count: sources.len(),
                limit: self.config.max_source_functions,
            });
        }

        let mut identities = BTreeSet::new();
        for source in sources {
            validate_function(source, self.config)?;
            if !identities.insert(source.identity.clone()) {
                return Err(HomologyError::DuplicateIdentity(source.identity.clone()));
            }
        }

        let mut canonical: Vec<_> = sources.iter().collect();
        canonical.sort_by(|a, b| a.identity.cmp(&b.identity));
        Ok(canonical
            .into_iter()
            .map(|source| self.query_one(source))
            .collect())
    }

    fn query_one(&self, source: &KnownFunction) -> HomologyResult {
        let normalized = normalize_body(&source.words);
        let total_anchors = normalized.len() - self.config.ngram_words + 1;
        let mut votes: BTreeMap<(usize, usize), usize> = BTreeMap::new();

        for source_offset in 0..=normalized.len() - self.config.ngram_words {
            let source_anchor = &normalized[source_offset..source_offset + self.config.ngram_words];
            let Some(occurrences) = self.postings.get(&anchor_key(source_anchor)) else {
                continue;
            };
            if occurrences.len() > self.config.max_postings_per_anchor {
                continue;
            }
            for occurrence in occurrences {
                let Some(target_start) = occurrence.word_offset.checked_sub(source_offset) else {
                    continue;
                };
                let region = &self.regions[occurrence.region];
                let Some(target_end) = target_start.checked_add(normalized.len()) else {
                    continue;
                };
                if target_end > region.normalized.len() {
                    continue;
                }
                let target_anchor = &region.normalized
                    [occurrence.word_offset..occurrence.word_offset + self.config.ngram_words];
                if target_anchor == source_anchor {
                    *votes.entry((occurrence.region, target_start)).or_default() += 1;
                }
            }
        }

        let best_anchor_votes = votes.values().copied().max().unwrap_or(0);
        let mut full_matches = Vec::new();
        for ((region_index, target_start), anchor_votes) in votes {
            if anchor_votes < self.config.min_anchor_votes {
                continue;
            }
            let region = &self.regions[region_index];
            let target_end = target_start + normalized.len();
            if region.normalized[target_start..target_end] == normalized {
                full_matches.push((region_index, target_start, anchor_votes));
            }
        }

        full_matches.sort_by(|a, b| {
            let left = &self.regions[a.0].input;
            let right = &self.regions[b.0].input;
            (&left.bank, left.va_start, a.1).cmp(&(&right.bank, right.va_start, b.1))
        });

        if full_matches.len() == 1 {
            let (region_index, target_start, anchor_votes) = full_matches[0];
            let region = &self.regions[region_index].input;
            let target_va = region.va_start + (target_start as u32) * 4;
            return HomologyResult::Candidate(HomologyCandidate {
                source_identity: source.identity.clone(),
                source: CodeLocation {
                    bank: source.bank.clone(),
                    va: source.va_start,
                },
                target: CodeLocation {
                    bank: region.bank.clone(),
                    va: target_va,
                },
                word_len: source.words.len(),
                anchor_votes,
                total_anchors,
            });
        }

        if full_matches.len() > 1 {
            let locations = full_matches
                .into_iter()
                .map(|(region_index, target_start, _)| {
                    let region = &self.regions[region_index].input;
                    CodeLocation {
                        bank: region.bank.clone(),
                        va: region.va_start + (target_start as u32) * 4,
                    }
                })
                .collect();
            return HomologyResult::Ambiguous {
                source_identity: source.identity.clone(),
                locations,
            };
        }

        HomologyResult::Unmatched {
            source_identity: source.identity.clone(),
            best_anchor_votes,
            total_anchors,
        }
    }
}

/// Convenience entry point for one-shot callers.
pub fn find_homology_candidates(
    sources: &[KnownFunction],
    regions: &[CodeRegion],
    config: HomologyConfig,
) -> Result<Vec<HomologyResult>, HomologyError> {
    HomologyIndex::build(regions, config)?.query(sources)
}

fn validate_config(config: HomologyConfig) -> Result<(), HomologyError> {
    if config.ngram_words == 0 {
        return Err(HomologyError::InvalidConfig("ngram_words must be nonzero"));
    }
    if config.min_function_words < config.ngram_words {
        return Err(HomologyError::InvalidConfig(
            "min_function_words must cover one n-gram",
        ));
    }
    if config.max_function_words < config.min_function_words {
        return Err(HomologyError::InvalidConfig(
            "max_function_words must not be smaller than min_function_words",
        ));
    }
    if config.max_postings_per_anchor == 0 || config.min_anchor_votes == 0 {
        return Err(HomologyError::InvalidConfig(
            "anchor posting and vote limits must be nonzero",
        ));
    }
    Ok(())
}

fn validate_regions(regions: &[CodeRegion]) -> Result<(), HomologyError> {
    let mut previous: Option<(&str, u32)> = None;
    for region in regions {
        if region.bank.is_empty() {
            return Err(HomologyError::EmptyBank);
        }
        if region.va_start & 3 != 0 {
            return Err(HomologyError::UnalignedAddress {
                bank: region.bank.clone(),
                va: region.va_start,
            });
        }
        if region.words.is_empty() {
            return Err(HomologyError::EmptyRegion {
                bank: region.bank.clone(),
                va_start: region.va_start,
            });
        }
        let byte_len = region
            .words
            .len()
            .checked_mul(4)
            .and_then(|len| u32::try_from(len).ok());
        let Some(end) = byte_len.and_then(|len| region.va_start.checked_add(len)) else {
            return Err(HomologyError::AddressOverflow {
                bank: region.bank.clone(),
                va_start: region.va_start,
                words: region.words.len(),
            });
        };
        if let Some((bank, previous_end)) = previous {
            if bank == region.bank && region.va_start < previous_end {
                return Err(HomologyError::OverlappingRegions {
                    bank: region.bank.clone(),
                    va: region.va_start,
                });
            }
        }
        previous = Some((&region.bank, end));
    }
    Ok(())
}

fn validate_function(
    function: &KnownFunction,
    config: HomologyConfig,
) -> Result<(), HomologyError> {
    if function.identity.is_empty() {
        return Err(HomologyError::EmptyIdentity);
    }
    if function.bank.is_empty() {
        return Err(HomologyError::EmptyBank);
    }
    if function.va_start & 3 != 0 {
        return Err(HomologyError::UnalignedAddress {
            bank: function.bank.clone(),
            va: function.va_start,
        });
    }
    if function.words.len() < config.min_function_words {
        return Err(HomologyError::FunctionTooShort {
            identity: function.identity.clone(),
            words: function.words.len(),
            min: config.min_function_words,
        });
    }
    if function.words.len() > config.max_function_words {
        return Err(HomologyError::FunctionTooLong {
            identity: function.identity.clone(),
            words: function.words.len(),
            max: config.max_function_words,
        });
    }
    let byte_len = function
        .words
        .len()
        .checked_mul(4)
        .and_then(|len| u32::try_from(len).ok());
    if byte_len
        .and_then(|len| function.va_start.checked_add(len))
        .is_none()
    {
        return Err(HomologyError::AddressOverflow {
            bank: function.bank.clone(),
            va_start: function.va_start,
            words: function.words.len(),
        });
    }
    Ok(())
}

/// Normalize address-bearing MIPS instruction fields while retaining opcode,
/// register, function, and branch-topology structure.
///
/// This intentionally masks more than a linker relocation table would. Raw
/// ROMs do not carry that table, and address changes can occur in LUI/immediate
/// constructions, J-format targets, and load/store offsets. The information
/// loss is why collision handling and independent corroboration are required.
pub fn relocation_masked_word(word: u32) -> u32 {
    let opcode = word >> 26;
    match opcode {
        // j/jal pseudo-region targets are absolute address material.
        0x02 | 0x03 => word & 0xfc00_0000,
        // Immediate arithmetic/logical operations, including lui.
        0x08..=0x0f => word & 0xffff_0000,
        // Load/store and cache-family offsets may name relocated data.
        0x20..=0x3f => word & 0xffff_0000,
        // Branch displacements remain: they encode intra-body topology and
        // do not change when a whole unchanged function moves.
        _ => word,
    }
}

pub fn normalize_body(words: &[u32]) -> Vec<u32> {
    words.iter().copied().map(relocation_masked_word).collect()
}

fn anchor_key(words: &[u32]) -> AnchorKey {
    AnchorKey {
        first: stable_hash(words, 0xcbf2_9ce4_8422_2325),
        second: stable_hash(words, 0x8422_2325_cbf2_9ce4),
    }
}

fn stable_hash(words: &[u32], seed: u64) -> u64 {
    words.iter().fold(seed, |hash, word| {
        let mixed = hash ^ u64::from(*word);
        mixed.wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> HomologyConfig {
        HomologyConfig {
            ngram_words: 2,
            min_function_words: 6,
            max_function_words: 64,
            max_source_functions: 16,
            max_target_regions: 16,
            max_target_words: 1024,
            max_postings_per_anchor: 32,
            min_anchor_votes: 2,
        }
    }

    fn body(address_hi: u16, address_lo: u16, jal_target: u32) -> Vec<u32> {
        vec![
            0x3c08_0000 | u32::from(address_hi), // lui t0, address_hi
            0x2508_0000 | u32::from(address_lo), // addiu t0, t0, address_lo
            0x0c00_0000 | ((jal_target >> 2) & 0x03ff_ffff),
            0x0000_0000, // nop
            0x8d02_0010, // lw v0, 0x10(t0)
            0x2442_0001, // addiu v0, v0, 1
            0x03e0_0008, // jr ra
            0x0000_0000, // nop
        ]
    }

    fn source(words: Vec<u32>) -> KnownFunction {
        KnownFunction {
            identity: "known_fn".into(),
            bank: "old".into(),
            va_start: 0x8000_1000,
            words,
        }
    }

    #[test]
    fn address_changes_produce_only_a_candidate() {
        let old = source(body(0x8001, 0x2340, 0x8000_4000));
        let new = CodeRegion {
            bank: "new".into(),
            va_start: 0x8010_0000,
            words: body(0x8034, 0x5678, 0x8020_8000),
        };

        let result = find_homology_candidates(&[old], &[new], config()).unwrap();
        let HomologyResult::Candidate(candidate) = &result[0] else {
            panic!("expected unique candidate: {:?}", result[0]);
        };
        assert_eq!(candidate.target.bank, "new");
        assert_eq!(candidate.target.va, 0x8010_0000);
        assert_eq!(candidate.word_len, 8);
    }

    #[test]
    fn normalized_collisions_are_reported_as_ambiguous() {
        let old = source(body(0x8001, 0x2340, 0x8000_4000));
        let regions = vec![
            CodeRegion {
                bank: "a".into(),
                va_start: 0x8001_0000,
                words: body(0x8011, 0x1110, 0x8004_0000),
            },
            CodeRegion {
                bank: "b".into(),
                va_start: 0x8002_0000,
                words: body(0x8022, 0x2220, 0x8008_0000),
            },
        ];

        let result = find_homology_candidates(&[old], &regions, config()).unwrap();
        let HomologyResult::Ambiguous { locations, .. } = &result[0] else {
            panic!("expected ambiguity: {:?}", result[0]);
        };
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].bank, "a");
        assert_eq!(locations[1].bank, "b");
    }

    #[test]
    fn matching_prefix_does_not_bypass_full_body_validation() {
        let old = source(body(0x8001, 0x2340, 0x8000_4000));
        let mut changed = body(0x8034, 0x5678, 0x8020_8000);
        changed[5] = 0x0042_1021; // addu v0, v0, v0: structural body change
        let target = CodeRegion {
            bank: "new".into(),
            va_start: 0x8010_0000,
            words: changed,
        };

        let result = find_homology_candidates(&[old], &[target], config()).unwrap();
        assert!(matches!(result[0], HomologyResult::Unmatched { .. }));
    }

    #[test]
    fn a_match_cannot_cross_region_or_bank_boundaries() {
        let words = body(0x8001, 0x2340, 0x8000_4000);
        let old = source(words.clone());
        let regions = vec![
            CodeRegion {
                bank: "new".into(),
                va_start: 0x8010_0000,
                words: words[..4].to_vec(),
            },
            CodeRegion {
                bank: "other".into(),
                va_start: 0x8010_0010,
                words: words[4..].to_vec(),
            },
        ];

        let result = find_homology_candidates(&[old], &regions, config()).unwrap();
        assert!(matches!(result[0], HomologyResult::Unmatched { .. }));
    }

    #[test]
    fn overlapping_regions_are_rejected_before_indexing() {
        let words = body(0x8001, 0x2340, 0x8000_4000);
        let regions = vec![
            CodeRegion {
                bank: "same".into(),
                va_start: 0x8000_0000,
                words: words.clone(),
            },
            CodeRegion {
                bank: "same".into(),
                va_start: 0x8000_0010,
                words,
            },
        ];

        assert!(matches!(
            HomologyIndex::build(&regions, config()),
            Err(HomologyError::OverlappingRegions { .. })
        ));
    }

    #[test]
    fn source_length_and_address_bounds_are_rejected() {
        let target = CodeRegion {
            bank: "new".into(),
            va_start: 0x8010_0000,
            words: body(0x8034, 0x5678, 0x8020_8000),
        };
        let index = HomologyIndex::build(&[target], config()).unwrap();
        let short = source(vec![0; 5]);
        assert!(matches!(
            index.query(&[short]),
            Err(HomologyError::FunctionTooShort { .. })
        ));

        let overflowing = KnownFunction {
            va_start: 0xffff_fff0,
            ..source(body(0x8001, 0x2340, 0x8000_4000))
        };
        assert!(matches!(
            index.query(&[overflowing]),
            Err(HomologyError::AddressOverflow { .. })
        ));
    }

    #[test]
    fn repeated_query_order_is_byte_for_byte_deterministic() {
        let sources = vec![
            KnownFunction {
                identity: "z".into(),
                ..source(body(0x8001, 0x2340, 0x8000_4000))
            },
            KnownFunction {
                identity: "a".into(),
                ..source(vec![
                    0x27bd_fff0,
                    0xafbf_000c,
                    0x0085_2021,
                    0x0c00_1000,
                    0x0000_0000,
                    0x8fbf_000c,
                    0x03e0_0008,
                    0x27bd_0010,
                ])
            },
        ];
        let regions = vec![CodeRegion {
            bank: "new".into(),
            va_start: 0x8010_0000,
            words: body(0x8034, 0x5678, 0x8020_8000),
        }];
        let index = HomologyIndex::build(&regions, config()).unwrap();

        let first = index.query(&sources).unwrap();
        let second = index.query(&sources).unwrap();
        assert_eq!(first, second);
        assert!(matches!(first[0], HomologyResult::Unmatched { .. }));
        assert!(matches!(first[1], HomologyResult::Candidate(_)));
    }
}
