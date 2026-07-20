//! Emulator-neutral, bounded dynamic-discovery probe plans.
//!
//! A plan identifies the normalized ROM, scenario, and input timeline, then
//! asks a trace producer for narrowly targeted observations. Every run has
//! instruction, event, and emulated-time limits. Expected information gain is
//! only an ordering heuristic over unresolved questions: it is neither a
//! confidence score nor a claim that the resulting observations are complete.

use crate::trace::{NormalizedRomDigest, PiDmaDirection, WatchedValueWidth};
use serde::{Deserialize, Serialize};
use std::fmt;

const PI_DRAM_ADDRESS_SPACE_END: u32 = 0x0100_0000;

/// SHA-256 identity of the exact input timeline consumed by the scenario.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputTimelineDigest(String);

impl InputTimelineDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for InputTimelineDigest {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64 {
            return Err("input timeline SHA-256 must contain exactly 64 hex digits");
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("input timeline SHA-256 must use lowercase hexadecimal");
        }
        Ok(Self(value))
    }
}

impl Serialize for InputTimelineDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for InputTimelineDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioIdentity {
    pub scenario_id: String,
    pub input_timeline_id: String,
    pub input_timeline_sha256: InputTimelineDigest,
    /// Allows a plan to target a bounded slice of a longer deterministic run.
    pub start_emulated_time_ns: u64,
}

/// All three limits are mandatory and nonzero. A producer stops on the first
/// limit reached and reports that termination; reaching a limit says nothing
/// about unexplored program behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeBudget {
    pub max_instructions: u64,
    pub max_events: u64,
    pub max_emulated_time_ns: u64,
}

/// A half-open byte interval. Domain-specific validation is performed by the
/// containing probe target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AddressRange {
    pub start: u32,
    pub end: u32,
}

impl AddressRange {
    fn is_nonempty(self) -> bool {
        self.start < self.end
    }

    fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// `Any` is an explicit absence of a bank constraint. It must not be silently
/// replaced by a bank inferred from an overlapping VA.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum BankScope {
    Any,
    Known { bank: String },
}

impl BankScope {
    fn overlaps(&self, other: &Self) -> bool {
        matches!(self, Self::Any)
            || matches!(other, Self::Any)
            || matches!((self, other), (Self::Known { bank: left }, Self::Known { bank: right }) if left == right)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProbeAddress {
    pub pc: u32,
    pub bank: BankScope,
}

/// Filters use intersection semantics. Omitted PI fields mean “any value in
/// this domain,” still bounded by the plan's mandatory run budgets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeTarget {
    ExecutedPcRange {
        bank: BankScope,
        range: AddressRange,
    },
    IndirectSite {
        site: ProbeAddress,
    },
    PiDma {
        direction: Option<PiDmaDirection>,
        cart_range: Option<AddressRange>,
        dram_range: Option<AddressRange>,
    },
    WatchedWrite {
        watch_id: String,
        bank: BankScope,
        range: AddressRange,
        widths: Vec<WatchedValueWidth>,
    },
}

/// An ordinal scheduling estimate. Larger values run first. The unresolved
/// question is required provenance for why the estimate exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedInformationGain {
    pub priority: u32,
    pub unresolved_question: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub probe_id: String,
    pub target: ProbeTarget,
    pub expected_information_gain: ExpectedInformationGain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbePlan {
    pub normalized_rom_sha256: NormalizedRomDigest,
    pub scenario: ScenarioIdentity,
    pub budget: ProbeBudget,
    pub probes: Vec<Probe>,
}

/// A validated plan has canonical probe ordering and canonical width ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ValidatedProbePlan(ProbePlan);

impl ValidatedProbePlan {
    pub fn as_plan(&self) -> &ProbePlan {
        &self.0
    }

    pub fn into_plan(self) -> ProbePlan {
        self.0
    }
}

impl TryFrom<ProbePlan> for ValidatedProbePlan {
    type Error = ProbePlanError;

    fn try_from(plan: ProbePlan) -> Result<Self, Self::Error> {
        plan.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbePlanError {
    pub subject: String,
    pub message: String,
}

impl ProbePlanError {
    fn new(subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ProbePlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "probe plan {}: {}", self.subject, self.message)
    }
}

impl std::error::Error for ProbePlanError {}

fn validate_identifier(subject: &str, label: &str, value: &str) -> Result<(), ProbePlanError> {
    if value.is_empty() {
        return Err(ProbePlanError::new(
            subject,
            format!("{label} must not be empty"),
        ));
    }
    if value.trim() != value {
        return Err(ProbePlanError::new(
            subject,
            format!("{label} must not have leading or trailing whitespace"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(ProbePlanError::new(
            subject,
            format!("{label} must not contain control characters"),
        ));
    }
    Ok(())
}

fn validate_bank(subject: &str, bank: &BankScope) -> Result<(), ProbePlanError> {
    if let BankScope::Known { bank } = bank {
        validate_identifier(subject, "bank", bank)?;
    }
    Ok(())
}

fn validate_range(subject: &str, label: &str, range: AddressRange) -> Result<(), ProbePlanError> {
    if !range.is_nonempty() {
        return Err(ProbePlanError::new(
            subject,
            format!(
                "{label} [0x{:08x}, 0x{:08x}) is empty or inverted",
                range.start, range.end
            ),
        ));
    }
    Ok(())
}

fn validate_instruction_range(subject: &str, range: AddressRange) -> Result<(), ProbePlanError> {
    validate_range(subject, "PC range", range)?;
    if range.start & 3 != 0 || range.end & 3 != 0 {
        return Err(ProbePlanError::new(
            subject,
            format!(
                "PC range [0x{:08x}, 0x{:08x}) is not four-byte aligned",
                range.start, range.end
            ),
        ));
    }
    Ok(())
}

fn validate_probe(probe: &mut Probe) -> Result<(), ProbePlanError> {
    let subject = format!("probe `{}`", probe.probe_id);
    validate_identifier(&subject, "probe_id", &probe.probe_id)?;
    if probe.expected_information_gain.priority == 0 {
        return Err(ProbePlanError::new(
            &subject,
            "expected information-gain priority must be nonzero",
        ));
    }
    validate_identifier(
        &subject,
        "unresolved_question",
        &probe.expected_information_gain.unresolved_question,
    )?;

    match &mut probe.target {
        ProbeTarget::ExecutedPcRange { bank, range } => {
            validate_bank(&subject, bank)?;
            validate_instruction_range(&subject, *range)?;
        }
        ProbeTarget::IndirectSite { site } => {
            validate_bank(&subject, &site.bank)?;
            if site.pc & 3 != 0 {
                return Err(ProbePlanError::new(
                    &subject,
                    format!("indirect site 0x{:08x} is not four-byte aligned", site.pc),
                ));
            }
        }
        ProbeTarget::PiDma {
            cart_range,
            dram_range,
            ..
        } => {
            if let Some(range) = cart_range {
                validate_range(&subject, "PI cartridge range", *range)?;
            }
            if let Some(range) = dram_range {
                validate_range(&subject, "PI DRAM range", *range)?;
                if range.end > PI_DRAM_ADDRESS_SPACE_END {
                    return Err(ProbePlanError::new(
                        &subject,
                        format!(
                            "PI DRAM range ends at 0x{:08x}, outside its 24-bit address space",
                            range.end
                        ),
                    ));
                }
            }
        }
        ProbeTarget::WatchedWrite {
            watch_id,
            bank,
            range,
            widths,
        } => {
            validate_identifier(&subject, "watch_id", watch_id)?;
            validate_bank(&subject, bank)?;
            validate_range(&subject, "watched-write range", *range)?;
            if widths.is_empty() {
                return Err(ProbePlanError::new(
                    &subject,
                    "watched-write widths must not be empty",
                ));
            }
            widths.sort();
            if widths.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(ProbePlanError::new(
                    &subject,
                    "watched-write widths contain a duplicate",
                ));
            }
            let largest_width = match widths.last().expect("nonempty checked above") {
                WatchedValueWidth::U8 => 1,
                WatchedValueWidth::U16 => 2,
                WatchedValueWidth::U32 => 4,
                WatchedValueWidth::U64 => 8,
            };
            // The filter admits every starting address below `end`; ensure a
            // maximum-width event at the final admitted address cannot wrap.
            (range.end - 1).checked_add(largest_width).ok_or_else(|| {
                ProbePlanError::new(&subject, "watched-write address plus width overflows u32")
            })?;
        }
    }
    Ok(())
}

fn optional_ranges_overlap(left: Option<AddressRange>, right: Option<AddressRange>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.overlaps(right),
        _ => true,
    }
}

fn targets_overlap(left: &ProbeTarget, right: &ProbeTarget) -> bool {
    match (left, right) {
        (
            ProbeTarget::ExecutedPcRange {
                bank: left_bank,
                range: left_range,
            },
            ProbeTarget::ExecutedPcRange {
                bank: right_bank,
                range: right_range,
            },
        ) => left_bank.overlaps(right_bank) && left_range.overlaps(*right_range),
        (ProbeTarget::IndirectSite { site: left }, ProbeTarget::IndirectSite { site: right }) => {
            left.pc == right.pc && left.bank.overlaps(&right.bank)
        }
        (
            ProbeTarget::PiDma {
                direction: left_direction,
                cart_range: left_cart,
                dram_range: left_dram,
            },
            ProbeTarget::PiDma {
                direction: right_direction,
                cart_range: right_cart,
                dram_range: right_dram,
            },
        ) => {
            (left_direction.is_none()
                || right_direction.is_none()
                || left_direction == right_direction)
                && optional_ranges_overlap(*left_cart, *right_cart)
                && optional_ranges_overlap(*left_dram, *right_dram)
        }
        (
            ProbeTarget::WatchedWrite {
                watch_id: left_id,
                bank: left_bank,
                range: left_range,
                widths: left_widths,
            },
            ProbeTarget::WatchedWrite {
                watch_id: right_id,
                bank: right_bank,
                range: right_range,
                widths: right_widths,
            },
        ) => {
            left_id == right_id
                && left_bank.overlaps(right_bank)
                && left_range.overlaps(*right_range)
                && left_widths.iter().any(|width| right_widths.contains(width))
        }
        _ => false,
    }
}

impl ProbePlan {
    /// Validate and canonicalize a plan. Invalid or overlapping requests are
    /// rejected instead of being merged, because merging would erase the
    /// provenance and information-gain rationale of one request.
    pub fn validate(mut self) -> Result<ValidatedProbePlan, ProbePlanError> {
        validate_identifier("scenario", "scenario_id", &self.scenario.scenario_id)?;
        validate_identifier(
            "scenario",
            "input_timeline_id",
            &self.scenario.input_timeline_id,
        )?;
        if self.budget.max_instructions == 0
            || self.budget.max_events == 0
            || self.budget.max_emulated_time_ns == 0
        {
            return Err(ProbePlanError::new(
                "budget",
                "instruction, event, and emulated-time limits must all be nonzero",
            ));
        }
        self.scenario
            .start_emulated_time_ns
            .checked_add(self.budget.max_emulated_time_ns)
            .ok_or_else(|| {
                ProbePlanError::new("budget", "scenario start plus time budget overflows u64")
            })?;
        if self.probes.is_empty() {
            return Err(ProbePlanError::new(
                "probes",
                "at least one probe is required",
            ));
        }

        // Identifier order makes all validation failures deterministic under
        // permutations of the same input plan.
        self.probes
            .sort_by(|left, right| left.probe_id.cmp(&right.probe_id));
        for pair in self.probes.windows(2) {
            if pair[0].probe_id == pair[1].probe_id {
                return Err(ProbePlanError::new(
                    format!("probe `{}`", pair[0].probe_id),
                    "duplicate probe_id",
                ));
            }
        }
        for probe in &mut self.probes {
            validate_probe(probe)?;
        }

        for left_index in 0..self.probes.len() {
            for right_index in (left_index + 1)..self.probes.len() {
                let left = &self.probes[left_index];
                let right = &self.probes[right_index];
                if targets_overlap(&left.target, &right.target) {
                    return Err(ProbePlanError::new(
                        format!("probes `{}` and `{}`", left.probe_id, right.probe_id),
                        "probe targets overlap or duplicate the same event domain",
                    ));
                }
            }
        }

        self.probes.sort_by(|left, right| {
            right
                .expected_information_gain
                .priority
                .cmp(&left.expected_information_gain.priority)
                .then_with(|| left.target.cmp(&right.target))
                .then_with(|| left.probe_id.cmp(&right.probe_id))
        });
        Ok(ValidatedProbePlan(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROM_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TIMELINE_DIGEST: &str =
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn target_probe(id: &str, priority: u32, target: ProbeTarget) -> Probe {
        Probe {
            probe_id: id.to_string(),
            target,
            expected_information_gain: ExpectedInformationGain {
                priority,
                unresolved_question: format!("resolve {id}"),
            },
        }
    }

    fn plan(probes: Vec<Probe>) -> ProbePlan {
        ProbePlan {
            normalized_rom_sha256: NormalizedRomDigest::try_from(ROM_DIGEST.to_string()).unwrap(),
            scenario: ScenarioIdentity {
                scenario_id: "boot-to-attract".to_string(),
                input_timeline_id: "neutral-input-v1".to_string(),
                input_timeline_sha256: InputTimelineDigest::try_from(TIMELINE_DIGEST.to_string())
                    .unwrap(),
                start_emulated_time_ns: 0,
            },
            budget: ProbeBudget {
                max_instructions: 1_000_000,
                max_events: 10_000,
                max_emulated_time_ns: 5_000_000_000,
            },
            probes,
        }
    }

    fn pc(id: &str, priority: u32, start: u32, end: u32, bank: BankScope) -> Probe {
        target_probe(
            id,
            priority,
            ProbeTarget::ExecutedPcRange {
                bank,
                range: AddressRange { start, end },
            },
        )
    }

    #[test]
    fn canonicalizes_by_information_gain_then_target_and_width() {
        let watched = target_probe(
            "watched",
            10,
            ProbeTarget::WatchedWrite {
                watch_id: "callbacks".to_string(),
                bank: BankScope::Any,
                range: AddressRange {
                    start: 0x8000_2000,
                    end: 0x8000_2100,
                },
                widths: vec![WatchedValueWidth::U32, WatchedValueWidth::U8],
            },
        );
        let validated = plan(vec![
            pc("low", 1, 0x8000_1000, 0x8000_1020, BankScope::Any),
            watched,
            pc(
                "high",
                20,
                0x8000_3000,
                0x8000_3020,
                BankScope::Known {
                    bank: "resident".to_string(),
                },
            ),
        ])
        .validate()
        .unwrap();

        let probes = &validated.as_plan().probes;
        assert_eq!(
            probes
                .iter()
                .map(|probe| probe.probe_id.as_str())
                .collect::<Vec<_>>(),
            ["high", "watched", "low"]
        );
        let ProbeTarget::WatchedWrite { widths, .. } = &probes[1].target else {
            panic!("expected watched-write probe");
        };
        assert_eq!(widths, &[WatchedValueWidth::U8, WatchedValueWidth::U32]);
    }

    #[test]
    fn permutations_produce_identical_canonical_json() {
        let first = pc("first", 1, 0x8000_1000, 0x8000_1010, BankScope::Any);
        let second = target_probe(
            "second",
            2,
            ProbeTarget::IndirectSite {
                site: ProbeAddress {
                    pc: 0x8000_2000,
                    bank: BankScope::Any,
                },
            },
        );
        let left = serde_json::to_string(
            &plan(vec![first.clone(), second.clone()])
                .validate()
                .unwrap(),
        )
        .unwrap();
        let right = serde_json::to_string(&plan(vec![second, first]).validate().unwrap()).unwrap();
        assert_eq!(left, right);
        assert!(!left.contains("complete"));
    }

    #[test]
    fn rejects_duplicate_ids_and_semantic_overlaps() {
        let duplicate = plan(vec![
            pc("same", 1, 0x8000_1000, 0x8000_1010, BankScope::Any),
            pc("same", 2, 0x8000_2000, 0x8000_2010, BankScope::Any),
        ])
        .validate()
        .unwrap_err();
        assert!(duplicate.message.contains("duplicate probe_id"));

        let overlap = plan(vec![
            pc("all", 1, 0x8000_1000, 0x8000_1100, BankScope::Any),
            pc(
                "banked",
                2,
                0x8000_1080,
                0x8000_1200,
                BankScope::Known {
                    bank: "overlay".to_string(),
                },
            ),
        ])
        .validate()
        .unwrap_err();
        assert!(overlap.message.contains("overlap"));

        plan(vec![
            pc("left", 1, 0x8000_1000, 0x8000_1100, BankScope::Any),
            pc("right", 1, 0x8000_1100, 0x8000_1200, BankScope::Any),
        ])
        .validate()
        .unwrap();
    }

    #[test]
    fn rejects_overlapping_dma_filters_and_watched_widths() {
        let dma = |id, direction, cart_range| {
            target_probe(
                id,
                1,
                ProbeTarget::PiDma {
                    direction,
                    cart_range,
                    dram_range: None,
                },
            )
        };
        let error = plan(vec![
            dma("all-reads", Some(PiDmaDirection::CartToRdram), None),
            dma(
                "rom-reads",
                Some(PiDmaDirection::CartToRdram),
                Some(AddressRange {
                    start: 0x1000_0000,
                    end: 0x1010_0000,
                }),
            ),
        ])
        .validate()
        .unwrap_err();
        assert!(error.message.contains("overlap"));

        let write = |id, start, end, widths| {
            target_probe(
                id,
                1,
                ProbeTarget::WatchedWrite {
                    watch_id: "table".to_string(),
                    bank: BankScope::Any,
                    range: AddressRange { start, end },
                    widths,
                },
            )
        };
        let error = plan(vec![
            write("wide", 0x1000, 0x1100, vec![WatchedValueWidth::U32]),
            write("inside", 0x1080, 0x1200, vec![WatchedValueWidth::U32]),
        ])
        .validate()
        .unwrap_err();
        assert!(error.message.contains("overlap"));
    }

    #[test]
    fn disjoint_dma_directions_and_watched_widths_are_allowed() {
        let dma_read = target_probe(
            "read",
            1,
            ProbeTarget::PiDma {
                direction: Some(PiDmaDirection::CartToRdram),
                cart_range: None,
                dram_range: None,
            },
        );
        let dma_write = target_probe(
            "write",
            1,
            ProbeTarget::PiDma {
                direction: Some(PiDmaDirection::RdramToCart),
                cart_range: None,
                dram_range: None,
            },
        );
        let u16_write = target_probe(
            "u16",
            1,
            ProbeTarget::WatchedWrite {
                watch_id: "table".to_string(),
                bank: BankScope::Any,
                range: AddressRange {
                    start: 0x1000,
                    end: 0x1100,
                },
                widths: vec![WatchedValueWidth::U16],
            },
        );
        let u32_write = target_probe(
            "u32",
            1,
            ProbeTarget::WatchedWrite {
                watch_id: "table".to_string(),
                bank: BankScope::Any,
                range: AddressRange {
                    start: 0x1000,
                    end: 0x1100,
                },
                widths: vec![WatchedValueWidth::U32],
            },
        );
        plan(vec![dma_read, dma_write, u16_write, u32_write])
            .validate()
            .unwrap();
    }

    #[test]
    fn rejects_bad_ranges_alignment_and_address_overflow() {
        let empty = plan(vec![pc(
            "empty",
            1,
            0x8000_1000,
            0x8000_1000,
            BankScope::Any,
        )])
        .validate()
        .unwrap_err();
        assert!(empty.message.contains("empty or inverted"));

        let unaligned = plan(vec![pc(
            "unaligned",
            1,
            0x8000_1002,
            0x8000_1010,
            BankScope::Any,
        )])
        .validate()
        .unwrap_err();
        assert!(unaligned.message.contains("four-byte aligned"));

        let dram = target_probe(
            "dram",
            1,
            ProbeTarget::PiDma {
                direction: None,
                cart_range: None,
                dram_range: Some(AddressRange {
                    start: 0x00ff_0000,
                    end: 0x0100_0001,
                }),
            },
        );
        assert!(plan(vec![dram])
            .validate()
            .unwrap_err()
            .message
            .contains("24-bit"));

        let watched = target_probe(
            "overflow",
            1,
            ProbeTarget::WatchedWrite {
                watch_id: "table".to_string(),
                bank: BankScope::Any,
                range: AddressRange {
                    start: u32::MAX - 3,
                    end: u32::MAX,
                },
                widths: vec![WatchedValueWidth::U64],
            },
        );
        assert!(plan(vec![watched])
            .validate()
            .unwrap_err()
            .message
            .contains("overflows u32"));
    }

    #[test]
    fn rejects_zero_budgets_and_time_overflow() {
        let probe = pc("pc", 1, 0x8000_1000, 0x8000_1010, BankScope::Any);
        let mut zero = plan(vec![probe.clone()]);
        zero.budget.max_events = 0;
        assert!(zero.validate().unwrap_err().message.contains("nonzero"));

        let mut overflow = plan(vec![probe]);
        overflow.scenario.start_emulated_time_ns = u64::MAX - 5;
        overflow.budget.max_emulated_time_ns = 10;
        assert!(overflow
            .validate()
            .unwrap_err()
            .message
            .contains("overflows u64"));
    }

    #[test]
    fn rejects_ambiguous_timeline_digests_and_duplicate_widths() {
        assert!(InputTimelineDigest::try_from("F".repeat(64)).is_err());
        let duplicate_width = target_probe(
            "write",
            1,
            ProbeTarget::WatchedWrite {
                watch_id: "table".to_string(),
                bank: BankScope::Any,
                range: AddressRange {
                    start: 0x1000,
                    end: 0x1100,
                },
                widths: vec![WatchedValueWidth::U16, WatchedValueWidth::U16],
            },
        );
        assert!(plan(vec![duplicate_width])
            .validate()
            .unwrap_err()
            .message
            .contains("duplicate"));
    }

    #[test]
    fn rejects_blank_identifiers_and_zero_information_priority() {
        let mut blank = pc(" pc ", 1, 0x8000_1000, 0x8000_1010, BankScope::Any);
        assert!(plan(vec![blank.clone()])
            .validate()
            .unwrap_err()
            .message
            .contains("whitespace"));

        blank.probe_id = "pc".to_string();
        blank.expected_information_gain.priority = 0;
        assert!(plan(vec![blank])
            .validate()
            .unwrap_err()
            .message
            .contains("nonzero"));
    }

    #[test]
    fn overlap_failure_is_deterministic_under_permutation() {
        let left = pc("a", 1, 0x8000_1000, 0x8000_1100, BankScope::Any);
        let right = pc("b", 1, 0x8000_1080, 0x8000_1180, BankScope::Any);
        let first = plan(vec![left.clone(), right.clone()])
            .validate()
            .unwrap_err();
        let second = plan(vec![right, left]).validate().unwrap_err();
        assert_eq!(first, second);
    }

    #[test]
    fn normalized_rom_digest_survives_serialization() {
        let validated = plan(vec![pc("pc", 1, 0x8000_1000, 0x8000_1010, BankScope::Any)])
            .validate()
            .unwrap();
        let json = serde_json::to_string(&validated).unwrap();
        assert!(json.contains(ROM_DIGEST));
        assert!(json.contains(TIMELINE_DIGEST));
    }
}
