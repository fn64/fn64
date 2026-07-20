//! Held-out grading for recovered reference targets.
//!
//! This module is deliberately downstream of discovery. It accepts recovered
//! references and a decompilation key, then measures whether each target is an
//! exact symbol in that key. Nothing here can add a root, fact, mapping, or
//! proof to discovery.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The reference-carrying mechanism that produced one recovered relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    DirectCall,
    HiLoLoad,
    HiLoStore,
    HiLoAddress,
    JumpTableTarget,
    JumpTableStorage,
    GpLoad,
    GpStore,
    GpAddress,
}

impl ReferenceKind {
    pub const ALL: [Self; 9] = [
        Self::DirectCall,
        Self::HiLoLoad,
        Self::HiLoStore,
        Self::HiLoAddress,
        Self::JumpTableTarget,
        Self::JumpTableStorage,
        Self::GpLoad,
        Self::GpStore,
        Self::GpAddress,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::DirectCall => "direct_call",
            Self::HiLoLoad => "hi_lo_load",
            Self::HiLoStore => "hi_lo_store",
            Self::HiLoAddress => "hi_lo_address",
            Self::JumpTableTarget => "jump_table_target",
            Self::JumpTableStorage => "jump_table_storage",
            Self::GpLoad => "gp_load",
            Self::GpStore => "gp_store",
            Self::GpAddress => "gp_address",
        }
    }

    fn expected_target(self) -> ExpectedTarget {
        match self {
            Self::DirectCall | Self::JumpTableTarget => ExpectedTarget::Code,
            Self::HiLoLoad
            | Self::HiLoStore
            | Self::JumpTableStorage
            | Self::GpLoad
            | Self::GpStore
            | Self::GpAddress => ExpectedTarget::Data,
            Self::HiLoAddress => ExpectedTarget::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedTarget {
    Code,
    Data,
    Unknown,
}

/// One bank-qualified reference recovered before the held-out key is opened.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecoveredReference {
    pub bank: String,
    pub referrer: u32,
    pub target: u32,
    pub kind: ReferenceKind,
}

impl RecoveredReference {
    pub fn new(bank: impl Into<String>, referrer: u32, target: u32, kind: ReferenceKind) -> Self {
        Self {
            bank: bank.into(),
            referrer,
            target,
            kind,
        }
    }
}

/// What the supplied key can authoritatively grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySupport {
    /// `dump.toml` carries section and function extents, but neither actual
    /// relocation records nor data-symbol addresses. Exact function targets
    /// are therefore the only positive symbol matches.
    FunctionSymbolsOnly,
}

impl KeySupport {
    pub fn description(self) -> &'static str {
        match self {
            Self::FunctionSymbolsOnly => {
                "function-symbol addresses and function/section extents only; no relocation records or data-symbol addresses"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WrongReason {
    /// A code reference landed in section-covered bytes outside every known
    /// function body.
    DataWhereCodeExpected,
    /// A data reference landed exactly on a known function symbol.
    CodeWhereDataExpected,
    /// The target is inside a known function, but not at its symbol address.
    FunctionInteriorNearMiss,
    /// The target is covered by a section but has no function symbol. With
    /// no data symbols in the key this cannot be credited as a symbol match.
    UnsymbolizedSectionAddress,
    /// No section in the key covers the target.
    OutsideKeyCoverage,
}

impl WrongReason {
    pub const ALL: [Self; 5] = [
        Self::DataWhereCodeExpected,
        Self::CodeWhereDataExpected,
        Self::FunctionInteriorNearMiss,
        Self::UnsymbolizedSectionAddress,
        Self::OutsideKeyCoverage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::DataWhereCodeExpected => "data_where_code_expected",
            Self::CodeWhereDataExpected => "code_where_data_expected",
            Self::FunctionInteriorNearMiss => "function_interior_near_miss",
            Self::UnsymbolizedSectionAddress => "unsymbolized_section_address",
            Self::OutsideKeyCoverage => "outside_key_coverage",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceGrade {
    Correct,
    Wrong(WrongReason),
}

#[derive(Debug, Clone)]
pub struct GradedReference {
    pub reference: RecoveredReference,
    pub grade: ReferenceGrade,
}

#[derive(Debug, Clone)]
pub struct RelocationGradeReport {
    pub recovered: usize,
    pub correct: usize,
    pub wrong: usize,
    pub per_kind: BTreeMap<ReferenceKind, (usize, usize)>,
    pub wrong_reasons: BTreeMap<WrongReason, usize>,
    pub references: Vec<GradedReference>,
}

impl RelocationGradeReport {
    pub fn misclassification_rate(&self) -> f64 {
        if self.recovered == 0 {
            0.0
        } else {
            self.wrong as f64 / self.recovered as f64
        }
    }
}

#[derive(Debug, Deserialize)]
struct DumpDoc {
    #[serde(default)]
    section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
struct SectionDoc {
    name: String,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
struct FunctionDoc {
    name: String,
    vram: u32,
    size: u32,
}

/// Grading-only view of a `dump.toml` key.
#[derive(Debug, Clone)]
pub struct RelocationKey {
    support: KeySupport,
    function_rows: usize,
    function_symbols: BTreeSet<u32>,
    function_ranges: Vec<(u32, u32)>,
    section_ranges: Vec<(u32, u32)>,
}

impl RelocationKey {
    pub fn support(&self) -> KeySupport {
        self.support
    }

    pub fn function_rows(&self) -> usize {
        self.function_rows
    }

    pub fn distinct_function_symbols(&self) -> usize {
        self.function_symbols.len()
    }

    pub fn section_count(&self) -> usize {
        self.section_ranges.len()
    }
}

/// Parse the held-out decomp key. Every function extent must be contained by
/// its section; malformed rows fail the entire grade instead of disappearing.
pub fn parse_relocation_key(text: &str) -> Result<RelocationKey, String> {
    let doc: DumpDoc = toml::from_str(text).map_err(|error| error.to_string())?;
    if doc.section.is_empty() {
        return Err("relocation grading key contains no sections".to_string());
    }

    let mut function_rows = 0usize;
    let mut function_symbols = BTreeSet::new();
    let mut function_ranges = Vec::new();
    let mut section_ranges = Vec::with_capacity(doc.section.len());
    for section in doc.section {
        let section_end = section
            .vram
            .checked_add(section.size)
            .ok_or_else(|| format!("section {:?} extent overflows u32", section.name))?;
        section_ranges.push((section.vram, section_end));
        for function in section.functions {
            function_rows += 1;
            let function_end = function
                .vram
                .checked_add(function.size)
                .ok_or_else(|| format!("function {:?} extent overflows u32", function.name))?;
            if function.vram < section.vram || function_end > section_end {
                return Err(format!(
                    "function {:?} [0x{:08x},0x{:08x}) lies outside section {:?} [0x{:08x},0x{:08x})",
                    function.name,
                    function.vram,
                    function_end,
                    section.name,
                    section.vram,
                    section_end
                ));
            }
            function_symbols.insert(function.vram);
            if function.size != 0 {
                function_ranges.push((function.vram, function_end));
            }
        }
    }
    if function_rows == 0 {
        return Err("relocation grading key contains no function symbols".to_string());
    }
    function_ranges.sort_unstable();
    section_ranges.sort_unstable();

    Ok(RelocationKey {
        support: KeySupport::FunctionSymbolsOnly,
        function_rows,
        function_symbols,
        function_ranges,
        section_ranges,
    })
}

fn contains(ranges: &[(u32, u32)], address: u32) -> bool {
    ranges
        .iter()
        .any(|&(start, end)| address >= start && address < end)
}

fn grade_one(reference: &RecoveredReference, key: &RelocationKey) -> ReferenceGrade {
    let exact_function = key.function_symbols.contains(&reference.target);
    if exact_function {
        return if reference.kind.expected_target() == ExpectedTarget::Data {
            ReferenceGrade::Wrong(WrongReason::CodeWhereDataExpected)
        } else {
            ReferenceGrade::Correct
        };
    }
    if contains(&key.function_ranges, reference.target) {
        return ReferenceGrade::Wrong(
            if reference.kind.expected_target() == ExpectedTarget::Data {
                WrongReason::CodeWhereDataExpected
            } else {
                WrongReason::FunctionInteriorNearMiss
            },
        );
    }
    if contains(&key.section_ranges, reference.target) {
        return ReferenceGrade::Wrong(
            if reference.kind.expected_target() == ExpectedTarget::Code {
                WrongReason::DataWhereCodeExpected
            } else {
                WrongReason::UnsymbolizedSectionAddress
            },
        );
    }
    ReferenceGrade::Wrong(WrongReason::OutsideKeyCoverage)
}

/// Grade a precomputed reference set without mutating or enriching it.
pub fn grade_references(
    references: &[RecoveredReference],
    key: &RelocationKey,
) -> RelocationGradeReport {
    let mut unique: Vec<_> = references
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    unique.sort();

    let mut correct = 0usize;
    let mut per_kind = BTreeMap::new();
    let mut wrong_reasons = BTreeMap::new();
    let mut graded = Vec::with_capacity(unique.len());
    for reference in unique {
        let grade = grade_one(&reference, key);
        let kind_tally = per_kind.entry(reference.kind).or_insert((0usize, 0usize));
        match grade {
            ReferenceGrade::Correct => {
                correct += 1;
                kind_tally.0 += 1;
            }
            ReferenceGrade::Wrong(reason) => {
                kind_tally.1 += 1;
                *wrong_reasons.entry(reason).or_insert(0) += 1;
            }
        }
        graded.push(GradedReference { reference, grade });
    }
    let recovered = graded.len();
    let wrong = recovered - correct;
    RelocationGradeReport {
        recovered,
        correct,
        wrong,
        per_kind,
        wrong_reasons,
        references: graded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = r#"
[[section]]
name = "text_and_data"
vram = 0x80000000
size = 0x100
functions = [
    { name = "known", vram = 0x80000020, size = 0x20 },
]
"#;

    fn grade(target: u32) -> ReferenceGrade {
        let key = parse_relocation_key(KEY).unwrap();
        let report = grade_references(
            &[RecoveredReference::new(
                "boot",
                0x8000_0000,
                target,
                ReferenceKind::DirectCall,
            )],
            &key,
        );
        report.references[0].grade.clone()
    }

    #[test]
    fn recovered_reference_to_known_symbol_is_correct() {
        assert_eq!(grade(0x8000_0020), ReferenceGrade::Correct);
    }

    #[test]
    fn recovered_reference_into_middle_of_data_is_wrong() {
        assert_eq!(
            grade(0x8000_0080),
            ReferenceGrade::Wrong(WrongReason::DataWhereCodeExpected)
        );
    }

    #[test]
    fn recovered_reference_near_miss_inside_function_is_wrong() {
        assert_eq!(
            grade(0x8000_0024),
            ReferenceGrade::Wrong(WrongReason::FunctionInteriorNearMiss)
        );
    }
}
