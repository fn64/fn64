//! The first-divergence report: compare fn64's own claimed register state
//! at a checkpoint PC against a reference runtime's ground-truth state at
//! that same PC (from the same starting conditions), and report the FIRST
//! field that disagrees -- not a fuzzy/aggregate score.
//!
//! "Checkpoint PC", not "single MIPS instruction", is the unit of comparison
//! here, and that is forced rather than chosen: fn64 executes at whole-
//! recompiled-function granularity, so there is no PC at which fn64 and an
//! instruction-stepping reference are simultaneously "mid-instruction" in a
//! comparable sense. See `docs/DESIGN.md` §1.0's no-mid-function-resume
//! finding for why this is a property of the runtime's shape and not a
//! limitation to be engineered away.
//!
//! This module is pure: every function here is a comparison over values the
//! caller supplies. Acquiring the reference state is deliberately not this
//! crate's business.

/// GPR names in R4300/o32 register-file order. Public hardware
/// documentation (the MIPS o32 ABI's register conventions), used to make a
/// diff readable by a human -- `sp` rather than `r29`.
pub const GPR_NAMES: [&str; 32] = [
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3", "t0", "t1", "t2", "t3", "t4", "t5", "t6",
    "t7", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "t8", "t9", "k0", "k1", "gp", "sp", "fp",
    "ra",
];

/// A plain MIPS register snapshot: the ground-truth side of a comparison.
///
/// Deliberately not tied to any particular producer -- it describes a
/// register set, nothing more. Whatever a caller can fill this in from (a
/// reference runtime, a trace replay, a hand-written fixture) is a valid
/// reference for [`compare_checkpoint`].
#[derive(Debug, Clone, PartialEq)]
pub struct RegisterSnapshot {
    pub pc: u32,
    /// Sign-extended 64-bit register contents, matching `RecompContext`'s
    /// own r-field convention (e.g. `sp` reads as `0xffffffff8008d098`).
    pub gprs: [u64; 32],
    pub cp0_status: u32,
    pub cp0_cause: u32,
    pub cp0_epc: u32,
    /// Steps the reference took to reach `pc`, when its producer tracks
    /// that; `0` when unknown. Diagnostic only -- never compared.
    pub steps: u64,
}

/// One fn64-side observation to check against the reference: the PC fn64's
/// executor claims to have reached, plus whatever register state it can
/// report at that instant. `None` entries in `gprs` mean "fn64 does not
/// track/expose this register at this checkpoint" -- such entries are
/// skipped during comparison rather than compared against zero, so an
/// intentionally partial checkpoint never manufactures a false divergence.
#[derive(Debug, Clone)]
pub struct Fn64Checkpoint {
    pub label: String,
    pub pc: u32,
    pub gprs: [Option<u64>; 32],
}

impl Fn64Checkpoint {
    pub fn new(label: impl Into<String>, pc: u32) -> Self {
        Fn64Checkpoint {
            label: label.into(),
            pc,
            gprs: [None; 32],
        }
    }

    pub fn with_gpr(mut self, index: usize, value: u64) -> Self {
        self.gprs[index] = Some(value);
        self
    }

    pub fn with_gprs_from(mut self, gprs: &[u64; 32]) -> Self {
        for (slot, &value) in self.gprs.iter_mut().zip(gprs.iter()) {
            *slot = Some(value);
        }
        self
    }
}

/// One field-level mismatch found at a checkpoint.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldDiff {
    Gpr {
        index: usize,
        name: &'static str,
        ours: u64,
        reference: u64,
    },
    Pc {
        ours: u32,
        reference: u32,
    },
}

impl std::fmt::Display for FieldDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FieldDiff::Gpr {
                name,
                ours,
                reference,
                ..
            } => write!(f, "{name}: ours=0x{ours:016x} reference=0x{reference:016x}"),
            FieldDiff::Pc { ours, reference } => {
                write!(f, "pc: ours=0x{ours:08x} reference=0x{reference:08x}")
            }
        }
    }
}

/// Outcome of comparing one [`Fn64Checkpoint`] against the reference.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckpointResult {
    /// Every field fn64 reported at this checkpoint matched the reference
    /// exactly. Not proof of full correctness (fn64 may report few fields,
    /// or the checkpoint PC may itself be wrong -- see `PcNotReached`) but
    /// a real, honest "no disagreement observed" result.
    Match,
    /// At least one field disagreed. `diffs` is ordered register-file-order
    /// (PC first, then r0..r31) so "first divergence" means "first entry
    /// in this list".
    Diverged { diffs: Vec<FieldDiff> },
    /// fn64 claims execution reached `pc`, but the reference -- run forward
    /// from the SAME starting conditions -- never reaches that PC at all
    /// within its budget. This is itself the strongest possible divergence
    /// signal (fn64 went somewhere the real machine provably does not go),
    /// reported as its own variant rather than folded into `Diverged`
    /// (there is no "reference value" to diff against).
    PcNotReached { pc: u32 },
}

impl CheckpointResult {
    pub fn is_match(&self) -> bool {
        matches!(self, CheckpointResult::Match)
    }
}

/// Compare one fn64 checkpoint against its ground-truth counterpart.
/// `reference` must be the reference runtime's state at `checkpoint.pc`
/// (same PC, same starting conditions) -- supplying a reference captured at
/// some other PC compares two unrelated instants and the `Pc` diff is the
/// only honest thing this can then report. Pure: no I/O, no subprocess.
pub fn compare_checkpoint(
    checkpoint: &Fn64Checkpoint,
    reference: &RegisterSnapshot,
) -> CheckpointResult {
    let mut diffs = Vec::new();

    if checkpoint.pc != reference.pc {
        diffs.push(FieldDiff::Pc {
            ours: checkpoint.pc,
            reference: reference.pc,
        });
    }

    for (index, (ours, name)) in checkpoint.gprs.iter().zip(GPR_NAMES.iter()).enumerate() {
        let Some(ours) = ours else { continue };
        // GPRs are compared as sign-extended 64-bit values: that is
        // `RecompContext`'s own r-field convention, so a reference producer
        // must sign-extend to match (e.g. sp reads as 0xffffffff8008d098,
        // not 0x8008d098). Comparing a zero-extended reference against a
        // sign-extended checkpoint reports every negative-address register
        // as a divergence.
        let reference_value = reference.gprs[index];
        if *ours != reference_value {
            diffs.push(FieldDiff::Gpr {
                index,
                name,
                ours: *ours,
                reference: reference_value,
            });
        }
    }

    if diffs.is_empty() {
        CheckpointResult::Match
    } else {
        CheckpointResult::Diverged { diffs }
    }
}

/// A full lockstep run's report: one [`CheckpointResult`] per checkpoint fn64
/// reported, in execution order, plus a convenience pointer to the FIRST
/// non-match -- the whole point of this crate. Later divergences are usually
/// consequences of the first one, so only the earliest is actionable.
#[derive(Debug)]
pub struct LockstepReport {
    pub checkpoints: Vec<(Fn64Checkpoint, CheckpointResult)>,
}

impl LockstepReport {
    pub fn new() -> Self {
        LockstepReport {
            checkpoints: Vec::new(),
        }
    }

    pub fn push(&mut self, checkpoint: Fn64Checkpoint, result: CheckpointResult) {
        self.checkpoints.push((checkpoint, result));
    }

    /// The first checkpoint (in execution order) that did not match, or
    /// `None` if every checkpoint matched (lockstep held for the whole run).
    pub fn first_divergence(&self) -> Option<(&Fn64Checkpoint, &CheckpointResult)> {
        self.checkpoints
            .iter()
            .find(|(_, result)| !result.is_match())
            .map(|(cp, result)| (cp, result))
    }

    /// Human-readable summary, reading as "@PC X, ours FIELD=Y ref FIELD=Z".
    pub fn summarize(&self) -> String {
        match self.first_divergence() {
            None => format!(
                "LOCKSTEP HELD: all {} checkpoint(s) matched the reference exactly, no divergence observed",
                self.checkpoints.len()
            ),
            Some((cp, CheckpointResult::PcNotReached { pc })) => format!(
                "FIRST DIVERGENCE @ checkpoint '{}': fn64 reached PC 0x{pc:08x}, but the reference \
                 (run forward from the same starting conditions) never reaches that PC at all",
                cp.label
            ),
            Some((cp, CheckpointResult::Diverged { diffs })) => {
                let first = diffs.first().expect("Diverged always has >=1 diff");
                format!(
                    "FIRST DIVERGENCE @ checkpoint '{}' (pc=0x{:08x}): {first}",
                    cp.label, cp.pc
                )
            }
            Some((_, CheckpointResult::Match)) => unreachable!("first_divergence only returns non-matches"),
        }
    }
}

impl Default for LockstepReport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(pc: u32, gprs: [u64; 32]) -> RegisterSnapshot {
        RegisterSnapshot {
            // cp0_*/steps are diagnostic-only; the comparator never reads
            // them, so a fixture leaving them zero is honest, not lazy.
            pc,
            gprs,
            cp0_status: 0,
            cp0_cause: 0,
            cp0_epc: 0,
            steps: 0,
        }
    }

    #[test]
    fn matching_checkpoint_reports_match() {
        let mut gprs = [0u64; 32];
        gprs[29] = 0x8008_d098;
        let checkpoint = Fn64Checkpoint::new("entry", 0x801187ac).with_gprs_from(&gprs);
        let result = compare_checkpoint(&checkpoint, &reference(0x801187ac, gprs));
        assert_eq!(result, CheckpointResult::Match);
    }

    #[test]
    fn a_single_wrong_register_is_reported_as_the_first_diff() {
        let mut ours = [0u64; 32];
        ours[29] = 0xBAD;
        let mut reference_gprs = [0u64; 32];
        reference_gprs[29] = 0x8008_d098;
        let checkpoint = Fn64Checkpoint::new("entry", 0x801187ac).with_gprs_from(&ours);
        let result = compare_checkpoint(&checkpoint, &reference(0x801187ac, reference_gprs));
        match result {
            CheckpointResult::Diverged { diffs } => {
                assert_eq!(diffs.len(), 1);
                assert_eq!(
                    diffs[0],
                    FieldDiff::Gpr {
                        index: 29,
                        name: "sp",
                        ours: 0xBAD,
                        reference: 0x8008_d098
                    }
                );
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }

    #[test]
    fn unset_gprs_are_skipped_not_compared_against_zero() {
        // A checkpoint that only reports r29 (sp) must not manufacture
        // spurious diffs for r2..r31 just because the reference has
        // nonzero values there.
        let mut reference_gprs = [0u64; 32];
        reference_gprs[2] = 0x1234; // v0, unset on our side
        reference_gprs[29] = 0x8008_d098;
        let checkpoint = Fn64Checkpoint::new("entry", 0x801187ac).with_gpr(29, 0x8008_d098);
        let result = compare_checkpoint(&checkpoint, &reference(0x801187ac, reference_gprs));
        assert_eq!(result, CheckpointResult::Match);
    }

    #[test]
    fn mismatched_pc_is_its_own_diff_kind() {
        let checkpoint = Fn64Checkpoint::new("entry", 0x801187b0);
        let result = compare_checkpoint(&checkpoint, &reference(0x801187ac, [0u64; 32]));
        match result {
            CheckpointResult::Diverged { diffs } => {
                assert_eq!(
                    diffs[0],
                    FieldDiff::Pc {
                        ours: 0x801187b0,
                        reference: 0x801187ac
                    }
                );
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }

    #[test]
    fn report_first_divergence_finds_the_earliest_non_match_in_order() {
        let mut report = LockstepReport::new();
        report.push(Fn64Checkpoint::new("cp0", 0x1000), CheckpointResult::Match);
        report.push(
            Fn64Checkpoint::new("cp1", 0x1004),
            CheckpointResult::Diverged {
                diffs: vec![FieldDiff::Gpr {
                    index: 4,
                    name: "a0",
                    ours: 1,
                    reference: 2,
                }],
            },
        );
        report.push(
            Fn64Checkpoint::new("cp2", 0x1008),
            CheckpointResult::PcNotReached { pc: 0x1008 },
        );

        let (cp, result) = report.first_divergence().expect("should find a divergence");
        assert_eq!(cp.label, "cp1");
        assert!(matches!(result, CheckpointResult::Diverged { .. }));
        assert!(report.summarize().contains("cp1"));
        assert!(report.summarize().contains("a0"));
    }

    #[test]
    fn report_with_all_matches_summarizes_as_held() {
        let mut report = LockstepReport::new();
        report.push(Fn64Checkpoint::new("cp0", 0x1000), CheckpointResult::Match);
        assert!(report.first_divergence().is_none());
        assert!(report.summarize().contains("LOCKSTEP HELD"));
    }
}
