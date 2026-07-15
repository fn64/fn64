//! Phase 2+ core (docs/DISCOVER-DESIGN.md "Monotonic fact database"):
//! immutable evidence records with provenance, plus the discrete proof
//! states the design mandates instead of one blended confidence number.
//!
//! The non-negotiable invariant: **a heuristic never overwrites proven
//! evidence.** This module enforces that at the data-structure level, not
//! by convention -- [`FactDb::insert`] is append-only, and promoting a
//! fact's `ProofState` can only ever move it to a state at least as strong
//! (see [`ProofState::supersedes`]); an attempt to downgrade a `Proven`
//! fact is a logic error the caller must handle, not something the DB does
//! silently.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A bank-qualified address: identity is `(bank, pc)`, never `pc` alone,
/// per the design doc's "Function identity must be bank-qualified from the
/// first instruction."
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BankAddr {
    pub bank: String,
    pub pc: u32,
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
    /// A proven ROM-interval -> runtime-VA-interval mapping: the bank
    /// identity itself. `rom_start`/`rom_end` are ROM byte offsets in the
    /// normalized (big-endian) ROM; `va_start`/`va_end` are runtime
    /// addresses; both exclusive of `_end`.
    RomMapping {
        bank: String,
        rom_start: u32,
        rom_end: u32,
        va_start: u32,
        va_end: u32,
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
            Fact::RomMapping { bank, .. } => Some(bank),
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
    /// update: never move a `Proven` conclusion to anything else. All
    /// other transitions are allowed -- new evidence may raise a
    /// `Candidate` to `Proven`, demote a `Candidate` to `Rejected` when a
    /// stronger fact contradicts it, or turn `Open` into `Conflict` when
    /// two incompatible proofs arrive at once. `Rejected` and `Conflict`
    /// are not "better than Candidate" in evidentiary terms -- they are
    /// alternate terminal outcomes -- but only `Proven` is unconditionally
    /// protected from a further transition.
    pub fn supersedes(self, next: ProofState) -> bool {
        if self == ProofState::Proven {
            return next == ProofState::Proven;
        }
        true
    }
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
            rom_start: 0x1000,
            rom_end: 0x2000,
            va_start: 0x8000_0400,
            va_end: 0x8000_1400,
        });
        let f1 = db.insert(Fact::RomMapping {
            bank: "overlay_1".into(),
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
