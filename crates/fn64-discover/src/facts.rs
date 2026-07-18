//! Phase 2+ core (docs/DISCOVER-DESIGN.md "Monotonic fact database"):
//! immutable evidence records with provenance, plus the discrete proof
//! states the design mandates instead of one blended confidence number.
//!
//! The non-negotiable invariant: **a heuristic never overwrites proven
//! evidence.** This module enforces that at the data-structure level, not
//! by convention -- [`FactDb::insert`] is append-only, and promoting a
//! fact's `ProofState` can only ever move it to a state at least as strong
//! (see [`ProofState::supersedes`]); an attempt to downgrade a `Proven`
//! fact is a logic error. Contradictory evidence may move it to `Conflict`,
//! which preserves rather than overwrites the disagreement.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A bank-qualified address: identity is `(bank, pc)`, never `pc` alone,
/// per the design doc's "Function identity must be bank-qualified from the
/// first instruction."
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BankAddr {
    pub bank: String,
    pub pc: u32,
}

/// Which ROM coordinate system a load-image range uses. N64 titles may put
/// executable bytes directly at physical cartridge offsets, or name files by
/// a virtual ROM (VROM) interval that a separate DMA table resolves to a
/// physical, possibly-compressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RomAddressSpace {
    Physical,
    Virtual,
}

/// Address spaces connected by a configurable Phase-2 range table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MappingAddressSpace {
    PhysicalRom,
    VirtualRom,
    Vram,
}

/// The deterministic Phase 3 provider that produced a function-entry claim.
/// These values are part of serialized provenance, so adding a provider must
/// add a new variant rather than reusing a free-form string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CandidateDetector {
    HardwareEntrypoint,
    JalTarget,
    IndirectCallTarget,
    ProloguePattern,
    TableDerived,
}

/// The exhaustive static construction behind an indirect-call target claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IndirectCallEvidenceKind {
    Constant,
    MemoryValueSet,
    JumpTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IndirectTransferState {
    Exhaustive,
    Bounded,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IndirectTransferKind {
    Constant,
    MemoryValueSet,
    JumpTable,
}

/// The prologue shape that justified a [`CandidateDetector::ProloguePattern`]
/// claim. A leaf claim requires a matched stack restore and `jr $ra`; a stack
/// adjustment by itself is deliberately insufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProloguePattern {
    SavesReturnAddress,
    LeafWithMatchedRestore,
}

/// Machine-readable evidence carried by every Phase 3 function-entry claim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FunctionEntryEvidence {
    /// The normalized ROM header entry executed after IPL3's hardware-fixed
    /// boot copy. This is authoritative reachability, not a heuristic
    /// prologue or tool claim.
    RomHeaderEntrypoint,
    DirectJal {
        call_site: BankAddr,
    },
    ResolvedJalr {
        call_site: BankAddr,
        construction_start: BankAddr,
    },
    ExhaustiveIndirectCall {
        call_site: BankAddr,
        kind: IndirectCallEvidenceKind,
        memory_sources: Vec<BankAddr>,
    },
    Prologue {
        stack_adjust: BankAddr,
        frame_size: u32,
        pattern: ProloguePattern,
        corroborating_site: BankAddr,
    },
    TableEntry {
        table: BankAddr,
        index: u32,
    },
}

impl BankAddr {
    pub fn new(bank: impl Into<String>, pc: u32) -> Self {
        Self {
            bank: bank.into(),
            pc,
        }
    }
}

/// One piece of raw, atomic evidence. Facts are never mutated or removed
/// once inserted -- only new facts are added, and derived conclusions
/// (banks, function ownership, proof states) are recomputed from the full
/// fact set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fact {
    /// `source` performs a direct `jal`/`j` to `target`, both bank-qualified.
    DirectCall { source: BankAddr, target: BankAddr },
    /// `bank` has a basic-block boundary starting at `pc`.
    BlockStart { bank: String, pc: u32 },
    /// The instruction at `site` loads from computed address `address`
    /// (used for HI/LO pairing and jump-table base discovery).
    LoadsFrom { site: BankAddr, address: u32 },
    /// A dynamic trace observed an indirect call/jump at `site` actually
    /// transferring control to `target`. Existence, not exhaustiveness --
    /// see docs/DISCOVER-DESIGN.md Phase 6.
    ObservedIndirectTarget {
        site: BankAddr,
        target: BankAddr,
        trace: String,
    },
    /// A dynamic trace observed the instruction at `site` execute. This is
    /// bank-scoped dynamic code-existence evidence -- strictly weaker than a
    /// proven owner or a CFG-reachability proof: it proves one word decoded
    /// and ran once in the named bank's activation, never that the trace's
    /// observed set is exhaustive (see `trace.rs`). `trace`/`sequence` pin
    /// this fact to one exact record of one exact capture so the evidence
    /// stays auditable.
    ObservedExecutedCode {
        site: BankAddr,
        trace: String,
        sequence: u64,
    },
    /// Static Phase 6 result for one reachable `jr`/`jalr`. Only an
    /// exhaustive record may contribute CFG successors; bounded/open records
    /// preserve the unresolved frontier without guessing.
    IndirectTransferAnalysis {
        site: BankAddr,
        via_call: bool,
        state: IndirectTransferState,
        kind: Option<IndirectTransferKind>,
        targets: Vec<u32>,
        memory_sources: Vec<u32>,
    },
    /// A proven ROM-interval -> runtime-VA-interval mapping: the bank
    /// identity itself. `rom_start`/`rom_end` are ROM byte offsets in the
    /// normalized (big-endian) ROM; `va_start`/`va_end` are runtime
    /// addresses; both exclusive of `_end`.
    RomMapping {
        bank: String,
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
        va_start: u32,
        va_end: u32,
    },
    /// A bank-qualified interval whose bytes are permitted to enter code
    /// discovery. Load-image mappings describe where bytes can be loaded;
    /// they do not imply that text, rodata, and data are all executable.
    ExecutableRange {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    /// One parsed record from an explicitly-located Phase-2 mapping table.
    /// This typed fact is the provenance for both VROM-to-physical-file
    /// mappings and ROM/VROM-to-VRAM load images: table identity and record
    /// index remain machine-readable rather than living only in prose.
    LoadImageTableRecord {
        table: String,
        bank: Option<String>,
        table_space: RomAddressSpace,
        table_offset: u32,
        index: u32,
        source_space: MappingAddressSpace,
        source_start: u32,
        source_end: u32,
        destination_space: MappingAddressSpace,
        destination_start: u32,
        destination_end: u32,
    },
    /// A code-pointer entry exposed by a descriptor table or vector that an
    /// earlier phase has already identified. Phase 3 consumes this fact; it
    /// never guesses table locations or semantics from raw bytes.
    TableEntry {
        table: BankAddr,
        index: u32,
        target: BankAddr,
    },
    /// One immutable Phase 3 provider result. `proposed_state` is the
    /// provider's own evidentiary strength; the `fn:<bank>:<pc>` conclusion
    /// is derived from all claims for the target by deterministic merge.
    FunctionEntryClaim {
        target: BankAddr,
        detector: CandidateDetector,
        evidence: FunctionEntryEvidence,
        proposed_state: ProofState,
    },
    /// Free-text provenance for a `RomMapping` or other claim -- e.g. "PI
    /// DMA descriptor at ROM 0x539a0, record 0" -- kept as a fact so the
    /// evidence trail survives independent of any derived report.
    Evidence { subject: BankAddr, note: String },
}

impl Fact {
    /// The bank this fact is scoped to, if it names one directly (some
    /// facts, like cross-bank `DirectCall`, don't have a single owner).
    pub fn primary_bank(&self) -> Option<&str> {
        match self {
            Fact::DirectCall { source, .. } => Some(&source.bank),
            Fact::BlockStart { bank, .. } => Some(bank),
            Fact::LoadsFrom { site, .. } => Some(&site.bank),
            Fact::ObservedIndirectTarget { site, .. } => Some(&site.bank),
            Fact::ObservedExecutedCode { site, .. } => Some(&site.bank),
            Fact::IndirectTransferAnalysis { site, .. } => Some(&site.bank),
            Fact::RomMapping { bank, .. } => Some(bank),
            Fact::ExecutableRange { bank, .. } => Some(bank),
            Fact::LoadImageTableRecord { bank, .. } => bank.as_deref(),
            Fact::TableEntry { table, .. } => Some(&table.bank),
            Fact::FunctionEntryClaim { target, .. } => Some(&target.bank),
            Fact::Evidence { subject, .. } => Some(&subject.bank),
        }
    }
}

/// Discrete proof states per docs/DISCOVER-DESIGN.md's "Result
/// classifications". Ordered weakest to strongest; [`ProofState::rank`]
/// gives the total order used to enforce monotonicity. `Conflict` and
/// `Rejected` are terminal in the sense that they require new evidence
/// (not mere heuristic re-scoring) to leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProofState {
    /// A required fact is still missing.
    Open,
    /// Produced by a heuristic or detector; no independent corroboration.
    Candidate,
    /// Corroborated by independent evidence, not yet authoritative.
    Supported,
    /// Contradicted by stronger evidence than what proposed it.
    Rejected,
    /// Incompatible evidence, or more than one valid interpretation.
    Conflict,
    /// Accepted by a defined proof rule; eligible for authoritative output.
    Proven,
}

impl ProofState {
    /// True if transitioning from `self` to `next` is a legal monotonic
    /// update: never move a `Proven` conclusion to anything weaker. A
    /// contradiction may move it to `Conflict`, which surfaces the new
    /// incompatible evidence without letting a heuristic win. All other
    /// transitions are allowed -- new evidence may raise a
    /// `Candidate` to `Proven`, demote a `Candidate` to `Rejected` when a
    /// stronger fact contradicts it, or turn `Open` into `Conflict` when
    /// two incompatible proofs arrive at once. `Rejected` and `Conflict`
    /// are not "better than Candidate" in evidentiary terms -- they are
    /// alternate terminal outcomes. A proven conclusion is protected from
    /// weakening while still allowing contradictory evidence to surface as
    /// `Conflict`.
    pub fn supersedes(self, next: ProofState) -> bool {
        if self == ProofState::Proven {
            return matches!(next, ProofState::Proven | ProofState::Conflict);
        }
        true
    }
}

/// Stable conclusion key for a bank-qualified function entry.
pub fn function_entry_subject(address: &BankAddr) -> String {
    format!("fn:{}:0x{:08x}", address.bank, address.pc)
}

/// Stable conclusion key for one upstream table/vector entry.
pub fn table_entry_subject(table: &BankAddr, index: u32) -> String {
    format!("table-entry:{}:0x{:08x}:{index}", table.bank, table.pc)
}

/// Stable conclusion key for one Phase-2 load-image/file-table record.
pub fn load_image_table_record_subject(table: &str, index: u32) -> String {
    format!("load-image-table:{table}:{index}")
}

/// Stable conclusion key for one bank-qualified executable interval.
pub fn executable_range_subject(bank: &str, va_start: u32, va_end: u32) -> String {
    format!("executable-range:{bank}:0x{va_start:08x}:0x{va_end:08x}")
}

/// Stable conclusion key for one bank-qualified word's dynamic
/// execution-observed evidence (`trace::fold_into_fact_db`'s subject).
pub fn observed_executed_code_subject(bank: &str, pc: u32) -> String {
    format!("observed-executed:{bank}:0x{pc:08x}")
}

/// A derived conclusion tracked with its proof state and the fact
/// indices that justify it. Conclusions are recomputed by proof rules
/// (see `banks.rs`), but the running record of "what did we conclude and
/// why" is itself kept append-only per subject so promotions are
/// auditable and demotions are refused, never silent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conclusion {
    pub subject: String,
    pub state: ProofState,
    /// Indices into `FactDb::facts` that justify this conclusion.
    pub justified_by: Vec<usize>,
    /// Human-readable rule name, e.g. "rom_mapping_from_boot_copy".
    pub rule: String,
}

/// The monotonic fact database: an append-only fact log plus a map of
/// current conclusions per subject. Facts are never removed or edited.
/// Conclusions may only move according to [`ProofState::supersedes`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactDb {
    facts: Vec<Fact>,
    conclusions: BTreeMap<String, Conclusion>,
}

/// Error returned when a proof-rule update would silently overwrite a
/// `Proven` conclusion with something weaker. Callers must either accept
/// this refusal (log it as a conflict-worthy anomaly) or add new evidence
/// strong enough to justify `Proven` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonotonicityViolation {
    pub subject: String,
    pub existing: ProofState,
    pub attempted: ProofState,
}

impl std::fmt::Display for MonotonicityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "refusing to downgrade proven fact for '{}' ({:?} -> {:?})",
            self.subject, self.existing, self.attempted
        )
    }
}

impl std::error::Error for MonotonicityViolation {}

impl FactDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a fact and return its stable index (used by conclusions'
    /// `justified_by`). Facts are never deduplicated destructively here --
    /// exact-duplicate facts are harmless and identical provenance from two
    /// independent detectors is itself useful corroborating evidence, not
    /// noise to be silently dropped.
    pub fn insert(&mut self, fact: Fact) -> usize {
        self.facts.push(fact);
        self.facts.len() - 1
    }

    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    /// Record or update a conclusion for `subject`, enforcing monotonicity.
    /// Returns the previous conclusion's state (if any) on success.
    pub fn conclude(
        &mut self,
        subject: impl Into<String>,
        state: ProofState,
        justified_by: Vec<usize>,
        rule: impl Into<String>,
    ) -> Result<Option<ProofState>, MonotonicityViolation> {
        let subject = subject.into();
        let previous = self.conclusions.get(&subject).map(|c| c.state);
        if let Some(prev) = previous {
            if !prev.supersedes(state) {
                return Err(MonotonicityViolation {
                    subject,
                    existing: prev,
                    attempted: state,
                });
            }
        }
        self.conclusions.insert(
            subject.clone(),
            Conclusion {
                subject,
                state,
                justified_by,
                rule: rule.into(),
            },
        );
        Ok(previous)
    }

    pub fn conclusion(&self, subject: &str) -> Option<&Conclusion> {
        self.conclusions.get(subject)
    }

    pub fn conclusions(&self) -> impl Iterator<Item = &Conclusion> {
        self.conclusions.values()
    }

    /// All `RomMapping` facts with `Proven` conclusions, i.e. accepted
    /// banks. This is the primary input to Phase 3+ (candidate harvesting
    /// operates within bank-qualified identity).
    pub fn proven_rom_mappings(&self) -> Vec<&Fact> {
        self.facts
            .iter()
            .filter(|f| {
                matches!(f, Fact::RomMapping { bank, .. }
                if self.conclusion(&format!("bank:{bank}"))
                    .map(|c| c.state == ProofState::Proven)
                    .unwrap_or(false))
            })
            .collect()
    }

    /// Proven executable intervals for `bank`, sorted by address. An empty
    /// result means no provider has separated text from the load image yet;
    /// callers may retain their legacy whole-image behavior but must not
    /// represent that fallback as proven executable coverage.
    pub fn proven_executable_ranges(&self, bank: &str) -> Vec<(u32, u32)> {
        let mut ranges: Vec<_> = self
            .facts
            .iter()
            .filter_map(|fact| {
                let Fact::ExecutableRange {
                    bank: fact_bank,
                    va_start,
                    va_end,
                } = fact
                else {
                    return None;
                };
                (fact_bank == bank
                    && self
                        .conclusion(&executable_range_subject(fact_bank, *va_start, *va_end))
                        .is_some_and(|conclusion| conclusion.state == ProofState::Proven))
                .then_some((*va_start, *va_end))
            })
            .collect();
        ranges.sort_unstable();
        ranges
    }

    /// Proven VROM-to-physical-ROM file mappings supplied by Phase 2. These
    /// are the byte-backing evidence used to materialize VROM load images;
    /// rejected/conflicting records cannot leak into detector input.
    pub fn proven_vrom_file_mappings(&self) -> Vec<(usize, &Fact)> {
        self.facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| {
                matches!(
                    fact,
                    Fact::LoadImageTableRecord {
                        table,
                        index,
                        source_space: MappingAddressSpace::VirtualRom,
                        destination_space: MappingAddressSpace::PhysicalRom,
                        ..
                    } if self
                        .conclusion(&load_image_table_record_subject(table, *index))
                        .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
                )
            })
            .collect()
    }

    /// Record an entry exposed by an already-identified table or vector.
    /// The caller supplies the table evidence's proof state; Phase 3 carries
    /// it forward and validates the target against discovered load images.
    pub fn record_table_entry(
        &mut self,
        table: BankAddr,
        index: u32,
        target: BankAddr,
        state: ProofState,
        rule: impl Into<String>,
    ) -> Result<usize, MonotonicityViolation> {
        let subject = table_entry_subject(&table, index);
        let fact = self.insert(Fact::TableEntry {
            table,
            index,
            target,
        });
        self.conclude(subject, state, vec![fact], rule)?;
        Ok(fact)
    }

    /// Function entries whose merged Phase 3 conclusion is authoritative.
    /// Candidate/supported/conflict entries remain visible in the database
    /// but cannot become CFG roots through this API.
    pub fn proven_function_entries(&self, bank: &str) -> Vec<u32> {
        let mut entries = BTreeSet::new();
        for fact in &self.facts {
            let Fact::FunctionEntryClaim { target, .. } = fact else {
                continue;
            };
            if target.bank == bank
                && self
                    .conclusion(&function_entry_subject(target))
                    .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
            {
                entries.insert(target.pc);
            }
        }
        entries.into_iter().collect()
    }

    /// Function-entry candidates suitable only for exploratory CFG coverage.
    /// This deliberately includes non-authoritative claims but never changes
    /// their proof state; callers must keep resulting owners/candidates out of
    /// exact-proof admission.
    pub fn candidate_function_entries(&self, bank: &str) -> Vec<u32> {
        let mut entries = BTreeSet::new();
        for fact in &self.facts {
            let Fact::FunctionEntryClaim { target, .. } = fact else {
                continue;
            };
            if target.bank != bank {
                continue;
            }
            let state = self
                .conclusion(&function_entry_subject(target))
                .map(|conclusion| conclusion.state);
            if matches!(
                state,
                Some(ProofState::Candidate | ProofState::Supported | ProofState::Proven)
            ) {
                entries.insert(target.pc);
            }
        }
        entries.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_is_append_only_and_indices_are_stable() {
        let mut db = FactDb::new();
        let i0 = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        let i1 = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0410,
        });
        assert_eq!(i0, 0);
        assert_eq!(i1, 1);
        assert_eq!(db.facts().len(), 2);
    }

    #[test]
    fn conclude_allows_strengthening_candidate_to_proven() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude(
            "fn:boot:0x80000400",
            ProofState::Candidate,
            vec![f],
            "heuristic",
        )
        .unwrap();
        db.conclude(
            "fn:boot:0x80000400",
            ProofState::Proven,
            vec![f],
            "direct_call_closure",
        )
        .unwrap();
        assert_eq!(
            db.conclusion("fn:boot:0x80000400").unwrap().state,
            ProofState::Proven
        );
    }

    #[test]
    fn conclude_refuses_to_downgrade_a_proven_fact() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude(
            "fn:boot:0x80000400",
            ProofState::Proven,
            vec![f],
            "direct_call_closure",
        )
        .unwrap();
        let err = db
            .conclude(
                "fn:boot:0x80000400",
                ProofState::Candidate,
                vec![f],
                "later_heuristic",
            )
            .unwrap_err();
        assert_eq!(err.existing, ProofState::Proven);
        assert_eq!(err.attempted, ProofState::Candidate);
        // The conclusion must be unchanged after the refused attempt.
        assert_eq!(
            db.conclusion("fn:boot:0x80000400").unwrap().state,
            ProofState::Proven
        );
    }

    #[test]
    fn conclude_surfaces_a_conflict_with_proven_evidence() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude("x", ProofState::Proven, vec![f], "proof")
            .unwrap();
        db.conclude("x", ProofState::Conflict, vec![f], "contradiction")
            .unwrap();
        assert_eq!(db.conclusion("x").unwrap().state, ProofState::Conflict);
    }

    #[test]
    fn table_entries_and_proven_function_roots_remain_bank_qualified() {
        let mut db = FactDb::new();
        let table = BankAddr::new("boot", 0x8000_1000);
        let target = BankAddr::new("overlay", 0x8010_0040);
        db.record_table_entry(
            table.clone(),
            3,
            target.clone(),
            ProofState::Proven,
            "proven_vector",
        )
        .unwrap();
        assert_eq!(
            db.conclusion(&table_entry_subject(&table, 3))
                .unwrap()
                .state,
            ProofState::Proven
        );

        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::TableDerived,
            evidence: FunctionEntryEvidence::TableEntry { table, index: 3 },
            proposed_state: ProofState::Proven,
        });
        db.conclude(
            function_entry_subject(&target),
            ProofState::Proven,
            vec![claim],
            "table_entry_merge",
        )
        .unwrap();
        assert_eq!(db.proven_function_entries("overlay"), vec![0x8010_0040]);
        assert!(db.proven_function_entries("boot").is_empty());
    }

    #[test]
    fn conclude_allows_proven_to_proven_reconfirmation() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude("x", ProofState::Proven, vec![f], "rule_a")
            .unwrap();
        db.conclude("x", ProofState::Proven, vec![f], "rule_b")
            .unwrap();
        assert_eq!(db.conclusion("x").unwrap().state, ProofState::Proven);
    }

    #[test]
    fn conclude_allows_candidate_to_rejected_on_contradiction() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude("x", ProofState::Candidate, vec![f], "heuristic")
            .unwrap();
        db.conclude(
            "x",
            ProofState::Rejected,
            vec![f],
            "contradicted_by_data_write",
        )
        .unwrap();
        assert_eq!(db.conclusion("x").unwrap().state, ProofState::Rejected);
    }

    #[test]
    fn proven_rom_mappings_filters_by_conclusion_not_presence() {
        let mut db = FactDb::new();
        let f0 = db.insert(Fact::RomMapping {
            bank: "boot".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x2000,
            va_start: 0x8000_0400,
            va_end: 0x8000_1400,
        });
        let f1 = db.insert(Fact::RomMapping {
            bank: "overlay_1".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x3000,
            rom_end: 0x4000,
            va_start: 0x8010_0000,
            va_end: 0x8010_1000,
        });
        db.conclude("bank:boot", ProofState::Proven, vec![f0], "boot_copy")
            .unwrap();
        db.conclude(
            "bank:overlay_1",
            ProofState::Candidate,
            vec![f1],
            "heuristic",
        )
        .unwrap();

        let proven = db.proven_rom_mappings();
        assert_eq!(proven.len(), 1);
        assert!(matches!(proven[0], Fact::RomMapping { bank, .. } if bank == "boot"));
    }
}
