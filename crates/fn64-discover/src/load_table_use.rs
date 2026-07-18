//! Descriptor-free recovery of overlay load tables from normalized static
//! load-use observations.
//!
//! This module deliberately starts after instruction/value analysis.  Its
//! input says that a word loaded from immutable mapped bytes reached one of
//! the semantic operands in the public N64 overlay sequence: instruction- and
//! data-cache invalidation, ROM-to-RDRAM DMA, or BSS clearing.  The recovery
//! performed here is purely structural:
//!
//! - every semantic role must resolve to one word in every record;
//! - all words must come from one immutable source bank;
//! - role offsets must be identical between records;
//! - record bases must form one constant-stride progression; and
//! - the values must satisfy the range relations in the public Programming
//!   Manual's overlay example.
//!
//! A plausible but incomplete shape remains [`RecoveryState::Candidate`].
//! Missing provenance is [`RecoveryState::Open`], and competing layouts are
//! [`RecoveryState::Ambiguous`].  None of those states is eligible to create a
//! proven ROM mapping.

use std::collections::{BTreeMap, BTreeSet};

/// The semantic sink reached by one word loaded from a possible overlay
/// descriptor record.
///
/// These are behavioral roles, not assumed field positions.  A preceding
/// data-flow pass derives them from the public overlay sequence.  This module
/// recovers the unique field offset associated with each role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlayFieldRole {
    RomStart,
    RomEnd,
    LoadStart,
    TextStart,
    TextEnd,
    DataStart,
    DataEnd,
    BssStart,
    BssEnd,
}

impl OverlayFieldRole {
    pub const ALL: [Self; 9] = [
        Self::RomStart,
        Self::RomEnd,
        Self::LoadStart,
        Self::TextStart,
        Self::TextEnd,
        Self::DataStart,
        Self::DataEnd,
        Self::BssStart,
        Self::BssEnd,
    ];
}

/// Immutable identity of a mapped byte source.  The integration layer should
/// use the canonical bank identity rather than a display label.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceBank(pub String);

impl SourceBank {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// Provenance for one normalized static word load.
///
/// `pointer + displacement` is retained so a consumer using a pointer biased
/// into the middle of a record is indistinguishable from one using its first
/// word after normalization.  `source_address` is repeated intentionally: the
/// producer and this proof boundary independently check the address equation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticWordLoad {
    pub role: OverlayFieldRole,
    pub source_bank: SourceBank,
    pub pointer: u32,
    pub displacement: i32,
    pub source_address: u32,
    pub value: u32,
    /// True when a store not proven disjoint may change the source word before
    /// the load.  Mutable table contents cannot establish a static ROM mapping.
    pub mutable_alias: bool,
}

impl StaticWordLoad {
    pub fn new(
        role: OverlayFieldRole,
        source_bank: impl Into<String>,
        pointer: u32,
        displacement: i32,
        source_address: u32,
        value: u32,
    ) -> Self {
        Self {
            role,
            source_bank: SourceBank::new(source_bank),
            pointer,
            displacement,
            source_address,
            value,
            mutable_alias: false,
        }
    }
}

/// All normalized loads feeding one invocation of an overlay loader helper.
/// `observation_id` is provenance only; recovery never assumes it is ordered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordUseObservation {
    pub observation_id: u64,
    pub call_site: u32,
    pub callee: u32,
    pub loads: Vec<StaticWordLoad>,
}

/// Unique byte offsets of the public overlay roles within one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayFieldOffsets {
    pub rom_start: u32,
    pub rom_end: u32,
    pub load_start: u32,
    pub text_start: u32,
    pub text_end: u32,
    pub data_start: u32,
    pub data_end: u32,
    pub bss_start: u32,
    pub bss_end: u32,
}

impl OverlayFieldOffsets {
    fn from_map(offsets: &BTreeMap<OverlayFieldRole, u32>) -> Option<Self> {
        Some(Self {
            rom_start: *offsets.get(&OverlayFieldRole::RomStart)?,
            rom_end: *offsets.get(&OverlayFieldRole::RomEnd)?,
            load_start: *offsets.get(&OverlayFieldRole::LoadStart)?,
            text_start: *offsets.get(&OverlayFieldRole::TextStart)?,
            text_end: *offsets.get(&OverlayFieldRole::TextEnd)?,
            data_start: *offsets.get(&OverlayFieldRole::DataStart)?,
            data_end: *offsets.get(&OverlayFieldRole::DataEnd)?,
            bss_start: *offsets.get(&OverlayFieldRole::BssStart)?,
            bss_end: *offsets.get(&OverlayFieldRole::BssEnd)?,
        })
    }

    pub fn get(self, role: OverlayFieldRole) -> u32 {
        match role {
            OverlayFieldRole::RomStart => self.rom_start,
            OverlayFieldRole::RomEnd => self.rom_end,
            OverlayFieldRole::LoadStart => self.load_start,
            OverlayFieldRole::TextStart => self.text_start,
            OverlayFieldRole::TextEnd => self.text_end,
            OverlayFieldRole::DataStart => self.data_start,
            OverlayFieldRole::DataEnd => self.data_end,
            OverlayFieldRole::BssStart => self.bss_start,
            OverlayFieldRole::BssEnd => self.bss_end,
        }
    }
}

/// Descriptor-table geometry established without a game descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredLoadTable {
    pub source_bank: SourceBank,
    pub callee: u32,
    pub table_address: u32,
    pub record_count: u32,
    pub record_stride: u32,
    pub field_offsets: OverlayFieldOffsets,
    /// Sorted record bases.  Keeping them makes the constant-stride proof
    /// independently checkable by the integration layer.
    pub record_addresses: Vec<u32>,
}

/// Exact bounds established by the preceding loop/value analysis.
///
/// Consecutive observed records alone cannot prove that the first and last
/// table records were seen. The producer must establish the loop's base,
/// count, and stride independently; this pass checks those bounds against the
/// normalized uses before it can return [`RecoveryState::Proven`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProvenAffineEnumeration {
    pub table_address: u32,
    pub record_count: u32,
    pub record_stride: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryState {
    Proven,
    Candidate,
    Open,
    Ambiguous,
}

/// Named reason a table was not proven.  Reasons are sorted and deduplicated
/// in [`RecoveryReport`] so input iteration cannot affect serialized output.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecoveryBlocker {
    NoObservations,
    EmptyObservation {
        observation_id: u64,
    },
    MixedCallees,
    MixedSourceBanks,
    MutableAlias {
        observation_id: u64,
        role: OverlayFieldRole,
    },
    AddressEquationMismatch {
        observation_id: u64,
        role: OverlayFieldRole,
    },
    UnalignedSource {
        observation_id: u64,
        address: u32,
    },
    DuplicateRoleSource {
        observation_id: u64,
        role: OverlayFieldRole,
    },
    MissingRole {
        observation_id: u64,
        role: OverlayFieldRole,
    },
    ConflictingDuplicateRecord {
        address: u32,
    },
    RoleOffsetChanged {
        role: OverlayFieldRole,
    },
    InsufficientDistinctRecords {
        found: u32,
    },
    EnumerationNotProven,
    EnumerationMismatch {
        expected_table_address: u32,
        expected_record_count: u32,
        expected_record_stride: u32,
    },
    NonConstantStride,
    StrideOverlapsRecord {
        stride: u32,
        record_width: u32,
    },
    OverlayRelationMismatch {
        observation_id: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    pub state: RecoveryState,
    pub table: Option<RecoveredLoadTable>,
    pub blockers: Vec<RecoveryBlocker>,
}

impl RecoveryReport {
    fn finish(
        state: RecoveryState,
        table: Option<RecoveredLoadTable>,
        blockers: impl IntoIterator<Item = RecoveryBlocker>,
    ) -> Self {
        let blockers: BTreeSet<_> = blockers.into_iter().collect();
        Self {
            state,
            table,
            blockers: blockers.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NormalizedRecord {
    observation_ids: BTreeSet<u64>,
    address: u32,
    offsets: BTreeMap<OverlayFieldRole, u32>,
    values: BTreeMap<OverlayFieldRole, u32>,
}

/// Recover one affine overlay table from observations already grouped as one
/// candidate loader family.
///
/// `rom_len` is the length of the normalized ROM address space used by the
/// recovered `RomStart`/`RomEnd` values.  Physical-vs-VROM interpretation is an
/// integration concern and must be supplied by the producer's typed source
/// domain; this module never guesses it from a number.
pub fn recover_affine_overlay_table(
    observations: &[RecordUseObservation],
    rom_len: u32,
    enumeration: Option<ProvenAffineEnumeration>,
) -> RecoveryReport {
    if observations.is_empty() {
        return RecoveryReport::finish(
            RecoveryState::Open,
            None,
            [RecoveryBlocker::NoObservations],
        );
    }

    let callees: BTreeSet<_> = observations.iter().map(|use_| use_.callee).collect();
    if callees.len() != 1 {
        return RecoveryReport::finish(RecoveryState::Open, None, [RecoveryBlocker::MixedCallees]);
    }

    if observations.iter().all(|use_| use_.loads.is_empty()) {
        return RecoveryReport::finish(
            RecoveryState::Candidate,
            None,
            observations
                .iter()
                .map(|use_| RecoveryBlocker::EmptyObservation {
                    observation_id: use_.observation_id,
                }),
        );
    }

    let banks: BTreeSet<_> = observations
        .iter()
        .flat_map(|use_| use_.loads.iter().map(|load| load.source_bank.clone()))
        .collect();
    if banks.len() != 1 {
        return RecoveryReport::finish(
            RecoveryState::Open,
            None,
            [RecoveryBlocker::MixedSourceBanks],
        );
    }
    let source_bank = banks.into_iter().next().expect("non-empty bank set");

    let mut hard_blockers = Vec::new();
    let mut candidate_blockers = Vec::new();
    let mut ambiguous_blockers = Vec::new();
    let mut normalized = Vec::new();

    for observation in observations {
        if observation.loads.is_empty() {
            candidate_blockers.push(RecoveryBlocker::EmptyObservation {
                observation_id: observation.observation_id,
            });
            continue;
        }

        let mut by_role: BTreeMap<OverlayFieldRole, Vec<&StaticWordLoad>> = BTreeMap::new();
        for load in &observation.loads {
            if load.mutable_alias {
                hard_blockers.push(RecoveryBlocker::MutableAlias {
                    observation_id: observation.observation_id,
                    role: load.role,
                });
            }
            if add_signed(load.pointer, load.displacement) != Some(load.source_address) {
                hard_blockers.push(RecoveryBlocker::AddressEquationMismatch {
                    observation_id: observation.observation_id,
                    role: load.role,
                });
            }
            if load.source_address & 3 != 0 {
                hard_blockers.push(RecoveryBlocker::UnalignedSource {
                    observation_id: observation.observation_id,
                    address: load.source_address,
                });
            }
            by_role.entry(load.role).or_default().push(load);
        }

        let mut unique = BTreeMap::new();
        for role in OverlayFieldRole::ALL {
            let Some(loads) = by_role.get(&role) else {
                candidate_blockers.push(RecoveryBlocker::MissingRole {
                    observation_id: observation.observation_id,
                    role,
                });
                continue;
            };
            let alternatives: BTreeSet<_> = loads
                .iter()
                .map(|load| (load.source_address, load.value))
                .collect();
            if alternatives.len() != 1 {
                ambiguous_blockers.push(RecoveryBlocker::DuplicateRoleSource {
                    observation_id: observation.observation_id,
                    role,
                });
                continue;
            }
            unique.insert(role, loads[0]);
        }

        if unique.len() != OverlayFieldRole::ALL.len() {
            continue;
        }
        let address = unique
            .values()
            .map(|load| load.source_address)
            .min()
            .expect("complete record has a load");
        let offsets = unique
            .iter()
            .map(|(&role, load)| (role, load.source_address - address))
            .collect();
        let values = unique
            .iter()
            .map(|(&role, load)| (role, load.value))
            .collect();
        normalized.push(NormalizedRecord {
            observation_ids: BTreeSet::from([observation.observation_id]),
            address,
            offsets,
            values,
        });
    }

    if !hard_blockers.is_empty() {
        return RecoveryReport::finish(RecoveryState::Open, None, hard_blockers);
    }
    if !ambiguous_blockers.is_empty() {
        return RecoveryReport::finish(RecoveryState::Ambiguous, None, ambiguous_blockers);
    }
    if !candidate_blockers.is_empty() {
        return RecoveryReport::finish(RecoveryState::Candidate, None, candidate_blockers);
    }

    normalized.sort_by_key(|record| record.address);
    let mut records: Vec<NormalizedRecord> = Vec::new();
    for record in normalized {
        if let Some(previous) = records
            .last_mut()
            .filter(|old| old.address == record.address)
        {
            if previous.offsets != record.offsets || previous.values != record.values {
                return RecoveryReport::finish(
                    RecoveryState::Ambiguous,
                    None,
                    [RecoveryBlocker::ConflictingDuplicateRecord {
                        address: record.address,
                    }],
                );
            }
            previous.observation_ids.extend(record.observation_ids);
        } else {
            records.push(record);
        }
    }

    if records.len() < 2 {
        return RecoveryReport::finish(
            RecoveryState::Candidate,
            None,
            [RecoveryBlocker::InsufficientDistinctRecords {
                found: records.len() as u32,
            }],
        );
    }

    let canonical_offsets = records[0].offsets.clone();
    let changed_roles: BTreeSet<_> = records
        .iter()
        .skip(1)
        .flat_map(|record| {
            OverlayFieldRole::ALL
                .into_iter()
                .filter(|role| record.offsets.get(role) != canonical_offsets.get(role))
        })
        .collect();
    if !changed_roles.is_empty() {
        return RecoveryReport::finish(
            RecoveryState::Ambiguous,
            None,
            changed_roles
                .into_iter()
                .map(|role| RecoveryBlocker::RoleOffsetChanged { role }),
        );
    }

    let deltas: Vec<u32> = records
        .windows(2)
        .map(|pair| pair[1].address - pair[0].address)
        .collect();
    let stride = deltas[0];
    if stride == 0 || deltas.iter().any(|delta| *delta != stride) {
        return RecoveryReport::finish(
            RecoveryState::Candidate,
            None,
            [RecoveryBlocker::NonConstantStride],
        );
    }
    let record_width = canonical_offsets
        .values()
        .copied()
        .max()
        .expect("complete record has offsets")
        .saturating_add(4);
    if stride < record_width {
        return RecoveryReport::finish(
            RecoveryState::Candidate,
            None,
            [RecoveryBlocker::StrideOverlapsRecord {
                stride,
                record_width,
            }],
        );
    }

    let mut relation_blockers = Vec::new();
    for record in &records {
        if !overlay_relations_hold(&record.values, rom_len) {
            relation_blockers.extend(record.observation_ids.iter().map(|observation_id| {
                RecoveryBlocker::OverlayRelationMismatch {
                    observation_id: *observation_id,
                }
            }));
        }
    }
    if !relation_blockers.is_empty() {
        return RecoveryReport::finish(RecoveryState::Candidate, None, relation_blockers);
    }

    let Some(enumeration) = enumeration else {
        return RecoveryReport::finish(
            RecoveryState::Candidate,
            None,
            [RecoveryBlocker::EnumerationNotProven],
        );
    };
    if enumeration.table_address != records[0].address
        || enumeration.record_count != records.len() as u32
        || enumeration.record_stride != stride
    {
        return RecoveryReport::finish(
            RecoveryState::Candidate,
            None,
            [RecoveryBlocker::EnumerationMismatch {
                expected_table_address: enumeration.table_address,
                expected_record_count: enumeration.record_count,
                expected_record_stride: enumeration.record_stride,
            }],
        );
    }

    let field_offsets = OverlayFieldOffsets::from_map(&canonical_offsets)
        .expect("complete normalized records contain every overlay role");
    let table_address = records[0].address;
    let record_addresses = records.iter().map(|record| record.address).collect();
    let table = RecoveredLoadTable {
        source_bank,
        callee: *callees.iter().next().expect("one callee"),
        table_address,
        record_count: records.len() as u32,
        record_stride: stride,
        field_offsets,
        record_addresses,
    };
    RecoveryReport::finish(RecoveryState::Proven, Some(table), [])
}

fn add_signed(base: u32, displacement: i32) -> Option<u32> {
    let value = i64::from(base) + i64::from(displacement);
    u32::try_from(value).ok()
}

fn overlay_relations_hold(values: &BTreeMap<OverlayFieldRole, u32>, rom_len: u32) -> bool {
    let get = |role| values.get(&role).copied();
    let (
        Some(rom_start),
        Some(rom_end),
        Some(load_start),
        Some(text_start),
        Some(text_end),
        Some(data_start),
        Some(data_end),
        Some(bss_start),
        Some(bss_end),
    ) = (
        get(OverlayFieldRole::RomStart),
        get(OverlayFieldRole::RomEnd),
        get(OverlayFieldRole::LoadStart),
        get(OverlayFieldRole::TextStart),
        get(OverlayFieldRole::TextEnd),
        get(OverlayFieldRole::DataStart),
        get(OverlayFieldRole::DataEnd),
        get(OverlayFieldRole::BssStart),
        get(OverlayFieldRole::BssEnd),
    )
    else {
        return false;
    };

    let Some(rom_bytes) = rom_end.checked_sub(rom_start) else {
        return false;
    };
    let Some(initialized_bytes) = data_end.checked_sub(load_start) else {
        return false;
    };
    rom_start < rom_end
        && rom_end <= rom_len
        && rom_bytes == initialized_bytes
        && is_cpu_virtual(load_start)
        && load_start == text_start
        && text_start < text_end
        && text_end == data_start
        && data_start <= data_end
        && data_end == bss_start
        && bss_start <= bss_end
        && [
            load_start, text_start, text_end, data_start, data_end, bss_start, bss_end,
        ]
        .into_iter()
        .all(|address| address & 3 == 0 && is_cpu_virtual(address))
}

fn is_cpu_virtual(address: u32) -> bool {
    (0x8000_0000..0xc000_0000).contains(&address)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALLEE: u32 = 0x8000_2000;
    const BANK: &str = "resident";
    const ROLE_OFFSETS: [(OverlayFieldRole, u32); 9] = [
        (OverlayFieldRole::RomStart, 0x00),
        (OverlayFieldRole::RomEnd, 0x04),
        (OverlayFieldRole::LoadStart, 0x08),
        (OverlayFieldRole::TextStart, 0x0c),
        (OverlayFieldRole::TextEnd, 0x10),
        (OverlayFieldRole::DataStart, 0x14),
        (OverlayFieldRole::DataEnd, 0x18),
        (OverlayFieldRole::BssStart, 0x1c),
        (OverlayFieldRole::BssEnd, 0x20),
    ];

    fn role_values(index: u32) -> BTreeMap<OverlayFieldRole, u32> {
        let rom_start = 0x0010_0000 + index * 0x4000;
        let rom_end = rom_start + 0x3000;
        let load_start = 0x8010_0000 + index * 0x10_000;
        let text_end = load_start + 0x2000;
        let data_end = load_start + 0x3000;
        let bss_end = data_end + 0x800;
        BTreeMap::from([
            (OverlayFieldRole::RomStart, rom_start),
            (OverlayFieldRole::RomEnd, rom_end),
            (OverlayFieldRole::LoadStart, load_start),
            (OverlayFieldRole::TextStart, load_start),
            (OverlayFieldRole::TextEnd, text_end),
            (OverlayFieldRole::DataStart, text_end),
            (OverlayFieldRole::DataEnd, data_end),
            (OverlayFieldRole::BssStart, data_end),
            (OverlayFieldRole::BssEnd, bss_end),
        ])
    }

    fn record(
        observation_id: u64,
        index: u32,
        record_address: u32,
        pointer_bias: u32,
    ) -> RecordUseObservation {
        let pointer = record_address + pointer_bias;
        let values = role_values(index);
        let loads = ROLE_OFFSETS
            .into_iter()
            .map(|(role, offset)| {
                StaticWordLoad::new(
                    role,
                    BANK,
                    pointer,
                    offset as i32 - pointer_bias as i32,
                    record_address + offset,
                    values[&role],
                )
            })
            .collect();
        RecordUseObservation {
            observation_id,
            call_site: 0x8000_3000 + observation_id as u32 * 4,
            callee: CALLEE,
            loads,
        }
    }

    fn records(count: u32, order: &[u32], pointer_bias: u32) -> Vec<RecordUseObservation> {
        assert_eq!(count as usize, order.len());
        order
            .iter()
            .enumerate()
            .map(|(ordinal, index)| {
                record(
                    ordinal as u64,
                    *index,
                    0x8005_0000 + *index * 0x24,
                    pointer_bias,
                )
            })
            .collect()
    }

    fn enumeration(count: u32) -> Option<ProvenAffineEnumeration> {
        Some(ProvenAffineEnumeration {
            table_address: 0x8005_0000,
            record_count: count,
            record_stride: 0x24,
        })
    }

    #[test]
    fn biased_record_pointers_recover_field_offsets_and_table_base() {
        let report = recover_affine_overlay_table(
            &records(5, &[0, 1, 2, 3, 4], 0x10),
            0x0200_0000,
            enumeration(5),
        );
        assert_eq!(report.state, RecoveryState::Proven);
        assert!(report.blockers.is_empty());
        let table = report.table.unwrap();
        assert_eq!(table.source_bank, SourceBank::new(BANK));
        assert_eq!(table.table_address, 0x8005_0000);
        assert_eq!(table.record_count, 5);
        assert_eq!(table.record_stride, 0x24);
        assert_eq!(table.field_offsets.rom_start, 0);
        assert_eq!(table.field_offsets.bss_end, 0x20);
    }

    #[test]
    fn four_records_in_reverse_iteration_order_have_identical_recovery() {
        let forward = recover_affine_overlay_table(
            &records(4, &[0, 1, 2, 3], 0),
            0x0200_0000,
            enumeration(4),
        );
        let reverse = recover_affine_overlay_table(
            &records(4, &[3, 2, 1, 0], 0x10),
            0x0200_0000,
            enumeration(4),
        );
        assert_eq!(forward.state, RecoveryState::Proven);
        assert_eq!(reverse.state, RecoveryState::Proven);
        assert_eq!(forward.table, reverse.table);
    }

    #[test]
    fn repeated_observation_of_the_same_record_is_deduplicated() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        let mut repeated = input[2].clone();
        repeated.observation_id = 99;
        repeated.call_site += 0x100;
        input.push(repeated);
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Proven);
        assert_eq!(report.table.unwrap().record_count, 4);
    }

    #[test]
    fn consecutive_subset_without_exact_loop_bounds_is_not_proven() {
        let input = records(4, &[0, 1, 2, 3], 0x10);
        let report = recover_affine_overlay_table(&input, 0x0200_0000, None);
        assert_eq!(report.state, RecoveryState::Candidate);
        assert_eq!(report.table, None);
        assert_eq!(report.blockers, vec![RecoveryBlocker::EnumerationNotProven]);

        let wrong_count = Some(ProvenAffineEnumeration {
            table_address: 0x8005_0000,
            record_count: 5,
            record_stride: 0x24,
        });
        let report = recover_affine_overlay_table(&input, 0x0200_0000, wrong_count);
        assert_eq!(report.state, RecoveryState::Candidate);
        assert!(matches!(
            report.blockers.as_slice(),
            [RecoveryBlocker::EnumerationMismatch {
                expected_record_count: 5,
                ..
            }]
        ));
    }

    #[test]
    fn incomplete_record_stays_candidate() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        input[2]
            .loads
            .retain(|load| load.role != OverlayFieldRole::BssEnd);
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Candidate);
        assert_eq!(report.table, None);
        assert!(report.blockers.contains(&RecoveryBlocker::MissingRole {
            observation_id: 2,
            role: OverlayFieldRole::BssEnd,
        }));
    }

    #[test]
    fn one_record_is_not_a_table_proof() {
        let report = recover_affine_overlay_table(
            &[record(7, 0, 0x8005_0000, 0x10)],
            0x0200_0000,
            Some(ProvenAffineEnumeration {
                table_address: 0x8005_0000,
                record_count: 1,
                record_stride: 0x24,
            }),
        );
        assert_eq!(report.state, RecoveryState::Candidate);
        assert_eq!(
            report.blockers,
            vec![RecoveryBlocker::InsufficientDistinctRecords { found: 1 }]
        );
    }

    #[test]
    fn nonconstant_stride_stays_candidate() {
        let input = vec![
            record(0, 0, 0x8005_0000, 0),
            record(1, 1, 0x8005_0024, 0),
            record(2, 2, 0x8005_0050, 0),
            record(3, 3, 0x8005_0074, 0),
        ];
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Candidate);
        assert_eq!(report.blockers, vec![RecoveryBlocker::NonConstantStride]);
    }

    #[test]
    fn mixed_source_bank_is_open() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        input[1].loads[0].source_bank = SourceBank::new("other");
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Open);
        assert_eq!(report.blockers, vec![RecoveryBlocker::MixedSourceBanks]);
    }

    #[test]
    fn mutable_alias_is_open() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        input[0].loads[3].mutable_alias = true;
        let role = input[0].loads[3].role;
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Open);
        assert_eq!(
            report.blockers,
            vec![RecoveryBlocker::MutableAlias {
                observation_id: 0,
                role,
            }]
        );
    }

    #[test]
    fn competing_sources_for_one_role_are_ambiguous() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        let mut alternative = input[0]
            .loads
            .iter()
            .find(|load| load.role == OverlayFieldRole::RomStart)
            .unwrap()
            .clone();
        alternative.source_address += 4;
        alternative.displacement += 4;
        alternative.value += 0x100;
        input[0].loads.push(alternative);
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Ambiguous);
        assert_eq!(
            report.blockers,
            vec![RecoveryBlocker::DuplicateRoleSource {
                observation_id: 0,
                role: OverlayFieldRole::RomStart,
            }]
        );
    }

    #[test]
    fn changing_a_role_offset_between_records_is_ambiguous() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        let load = input[3]
            .loads
            .iter_mut()
            .find(|load| load.role == OverlayFieldRole::BssEnd)
            .unwrap();
        load.source_address += 4;
        load.displacement += 4;
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Ambiguous);
        assert_eq!(
            report.blockers,
            vec![RecoveryBlocker::RoleOffsetChanged {
                role: OverlayFieldRole::BssEnd,
            }]
        );
    }

    #[test]
    fn values_that_do_not_describe_the_public_overlay_sequence_stay_candidate() {
        let mut input = records(4, &[0, 1, 2, 3], 0x10);
        let load = input[1]
            .loads
            .iter_mut()
            .find(|load| load.role == OverlayFieldRole::DataEnd)
            .unwrap();
        load.value += 4;
        let report = recover_affine_overlay_table(&input, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Candidate);
        assert_eq!(
            report.blockers,
            vec![RecoveryBlocker::OverlayRelationMismatch { observation_id: 1 }]
        );
    }

    #[test]
    fn mixed_callees_and_bad_address_equations_are_open() {
        let mut mixed = records(4, &[0, 1, 2, 3], 0x10);
        mixed[3].callee += 4;
        assert_eq!(
            recover_affine_overlay_table(&mixed, 0x0200_0000, enumeration(4)).state,
            RecoveryState::Open
        );

        let mut bad_address = records(4, &[0, 1, 2, 3], 0x10);
        bad_address[0].loads[0].source_address += 4;
        let report = recover_affine_overlay_table(&bad_address, 0x0200_0000, enumeration(4));
        assert_eq!(report.state, RecoveryState::Open);
        assert!(matches!(
            report.blockers.as_slice(),
            [RecoveryBlocker::AddressEquationMismatch { .. }]
        ));
    }
}
