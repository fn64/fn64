//! `FN64_PROFILE=1` -- one gate that arms everything and emits one
//! authoritative report.
//!
//! # Why this exists
//!
//! Getting one decomposition previously required arming FIVE gates
//! simultaneously (`FN64_PHASE_TIMING`, `FN64_EXECUTOR_SPLIT`,
//! `FN64_FRAME_CENSUS_POPULATIONS`, `FN64_RESUME_SPLIT`,
//! `FN64_FRAME_CENSUS_SEQUENCE`), and the committed benchmark script exported
//! none of them. Every partial arming produced a report that looked complete
//! and was not: a NOT-ARMED channel reads zero, and zero renders identically to
//! "this costs nothing".
//!
//! # The four truthiness conventions, which is why this cannot just set five
//! variables to `1`
//!
//! The constituent gates do not agree on what "on" means:
//!
//! | gate | predicate | `=0` means |
//! |---|---|---|
//! | `FN64_PHASE_TIMING` | `var_os().is_some()` | **ON** |
//! | `FN64_EXECUTOR_SPLIT` | `var_os().is_some()` | **ON** |
//! | `FN64_RESUME_SPLIT` | `var_os().is_some()` | **ON** |
//! | `FN64_FRAME_CENSUS` | `env_flag` | off |
//! | `FN64_FRAME_CENSUS_POPULATIONS` | `env_flag` | off |
//! | `FN64_FRAME_CENSUS_SEQUENCE` | integer count | off (0 fields) |
//!
//! So `FN64_EXECUTOR_SPLIT=0` **arms** the instrument while
//! `FN64_FRAME_CENSUS_POPULATIONS=0` disarms it: two conventions, opposite
//! meanings, identical-looking values. That is the same shape as the env gate
//! where an empty value read as ON and produced a fabricated 4.9x with both
//! lanes secretly identical.
//!
//! **Consequence: arming is verified BY EFFECT, never by echoing the variable.**
//! An echo cannot distinguish the two `=0`s -- it would report both as armed, or
//! both as off, and be wrong either way. [`verify`] instead asks each channel
//! whether it actually produced data.
//!
//! # Why it refuses rather than printing a subset
//!
//! A plausible partial report is worse than no report, because it gets believed.
//! If `FN64_PROFILE` is set and any constituent failed to arm, the process
//! **exits non-zero naming the missing gate**.
//!
//! The NOT-ARMED notice is emitted from a path that is **not gated on the flag
//! it warns about**. A warning behind its own gate is unreachable by
//! construction -- that trap cost two 25-minute runs, and
//! `scripts/byte-identity-1p5M.txt` documents a live instance of it.
//!
//! # Where this runs
//!
//! Composition happens in [`arm`], called from the same
//! `advance_virtual_time` seam that installs the frame census -- inside
//! `fn64-abi`, so `examples/wm2000-block-boot/src/main.rs` is never edited. That
//! file is read verbatim into `DISPATCH_SOURCE_SHA256` by `build.rs`, so adding
//! even one `env::var` line there would change the canonical program identity
//! and make a before/after comparison read exactly like a perf change.

use std::sync::OnceLock;

/// The report tag. Must also appear in `render-benchmark.zsh`'s output
/// allowlist -- that filter drops any tag not named in it, and a report the
/// watcher cannot see has already cost two runs and one lost census.
pub const TAG: &str = "[fn64-profile]";

/// The 60fps bar. Every row in the report is stated against this as well as
/// against its own parent.
pub const FRAME_BUDGET_MS: f64 = 1000.0 / 60.0;

/// One constituent channel that `FN64_PROFILE` composes.
pub struct Channel {
    /// The env var this channel arms.
    pub gate: &'static str,
    /// The value to set, correct for THIS gate's own parser.
    pub value: &'static str,
    /// What is lost if it fails to arm.
    pub provides: &'static str,
}

/// The gates `FN64_PROFILE` arms, each with a value its own parser accepts.
///
/// `FN64_FRAME_CENSUS_SEQUENCE` is a COUNT, not a flag: `1` would dump a single
/// field, which cannot show a period. 400 is enough to see a period-2 through
/// period-8 cycle and is what the bimodal `SfSfSfSf` finding was read from.
pub const CHANNELS: &[Channel] = &[
    Channel {
        gate: "FN64_FRAME_CENSUS",
        value: "1",
        provides: "per-field latency distribution (the 60fps bar itself)",
    },
    Channel {
        gate: "FN64_FRAME_CENSUS_POPULATIONS",
        value: "1",
        provides: "the fast/slow population split and every per-field counter",
    },
    Channel {
        gate: "FN64_PHASE_TIMING",
        value: "1",
        provides: "executor_ns, gfx_ns, RSP/RDP and the phase totals",
    },
    Channel {
        gate: "FN64_EXECUTOR_SPLIT",
        value: "1",
        provides: "the executor_ns decomposition and resume NET",
    },
    Channel {
        gate: "FN64_RESUME_SPLIT",
        value: "1",
        provides: "the resume NET decomposition (guest code vs graphics)",
    },
    Channel {
        gate: "FN64_FRAME_CENSUS_SEQUENCE",
        value: "400",
        provides: "the raw per-field sequence, which alone distinguishes \
                   regimes from a trend",
    },
    // Added after the first real RT64 run printed the three staging rows as
    // 0.000 with no warning -- the exact "an unarmed channel reads zero, and
    // zero is indistinguishable from 'this costs nothing'" failure this module
    // exists to prevent, reintroduced by omitting the gate from this list.
    // The staging memcpy is a live optimization target, so a silent zero here
    // would have sent someone to optimize a row the instrument never measured.
    Channel {
        gate: "FN64_DPC_COPY_CENSUS",
        value: "1",
        provides: "the RDP staging copy split (alloc / copy_in / copy_back) \
                   and the RSP instruction counts",
    },
];

/// Whether `FN64_PROFILE` is set to something meaning yes.
///
/// Uses the strict `env_flag` convention (`1|true|yes|on`), NOT
/// `var_os().is_some()`: this gate is new, so it gets the convention where `=0`
/// means off, which is what a reader expects.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("FN64_PROFILE").is_some_and(|v| {
            matches!(
                v.to_string_lossy().trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
}

/// Set every constituent gate, unless the operator set it explicitly.
///
/// # Why this must run before `main`
///
/// The three `is_some()` gates live in `thread_local!` `Cell`s whose
/// initializer reads the environment on **first access per thread**
/// (`lifecycle.rs:62,84,145`). Once any thread has touched `PHASE_TIMING`, that
/// thread's answer is fixed for the process's life. Arming from a runtime seam
/// such as `advance_virtual_time` is therefore too late: the executor has
/// already run steps, already read the cells, and already cached "off" -- and
/// the resulting report would show the gates set in `PROVENANCE` while every
/// counter read zero. That is precisely the plausible-looking partial report
/// this module exists to make impossible.
///
/// So arming is a **life-before-main constructor**. It runs before the runtime
/// starts, before any thread exists, and therefore before any gate cell can be
/// initialized.
///
/// An explicit operator value always wins: `FN64_PROFILE=1
/// FN64_FRAME_CENSUS_SEQUENCE=2000` is a legitimate request for a longer dump,
/// not a conflict to override.
pub fn arm() {
    if !enabled() {
        return;
    }
    static ARMED: OnceLock<()> = OnceLock::new();
    ARMED.get_or_init(|| {
        for channel in CHANNELS {
            if std::env::var_os(channel.gate).is_none() {
                // SAFETY: runs from a pre-main constructor, before any thread
                // other than the initial one exists and before any
                // `thread_local!` gate cell has been initialized. `set_var` is
                // unsound only when racing another thread's environment access,
                // and there is no other thread yet.
                unsafe { std::env::set_var(channel.gate, channel.value) };
            }
        }
    });
}

/// Run [`arm`] before `main`.
///
/// Placed in `.init_array` (ELF) / `__DATA,__mod_init_func` (Mach-O; this
/// machine's format). A `Once`-guarded [`arm`] is also called from the census
/// install path as a belt-and-braces fallback for any linker configuration
/// that drops the section -- calling twice is harmless, calling too late is
/// not, and a silent failure to arm is the failure mode with teeth.
#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(not(target_os = "macos"), link_section = ".init_array")]
static ARM_BEFORE_MAIN: extern "C" fn() = {
    extern "C" fn ctor() {
        arm();
    }
    ctor
};

/// A channel that did not arm, or armed but produced no data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotArmed {
    pub gate: &'static str,
    pub provides: &'static str,
    pub reason: Reason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The variable is not set at all.
    Unset,
    /// The variable is set but the channel produced no data. On a route that
    /// reaches rendering this means the gate's own parser rejected the value --
    /// the `=0`-arms-vs-disarms trap.
    NoData,
}

impl Reason {
    fn describe(self) -> &'static str {
        match self {
            Self::Unset => "variable not set",
            Self::NoData => {
                "variable IS set but the channel produced no data -- check the value against \
                 this gate's parser; some gates treat `0` as ON and others as OFF"
            }
        }
    }
}

/// Verify each channel BY EFFECT: did it actually produce data?
///
/// `witness` answers "did this gate's counters record anything", which is the
/// only question that distinguishes armed from not on a codebase where `=0`
/// means opposite things to different parsers. Echoing the variable cannot.
///
/// Callers pass a witness derived from the run's own counters, so a channel
/// that was armed but never reached (a route that renders nothing) is reported
/// as `NoData` rather than silently reading zero.
pub fn verify(witness: &dyn Fn(&str) -> bool) -> Vec<NotArmed> {
    CHANNELS
        .iter()
        .filter_map(|channel| {
            let set = std::env::var_os(channel.gate).is_some();
            let produced = witness(channel.gate);
            match (set, produced) {
                (_, true) => None,
                (false, false) => Some(NotArmed {
                    gate: channel.gate,
                    provides: channel.provides,
                    reason: Reason::Unset,
                }),
                (true, false) => Some(NotArmed {
                    gate: channel.gate,
                    provides: channel.provides,
                    reason: Reason::NoData,
                }),
            }
        })
        .collect()
}

/// The refusal notice for unarmed channels.
///
/// **Not gated on `FN64_PROFILE`.** This is emitted whenever a caller asks for
/// it, including when the profile flag itself is absent, because a warning
/// behind the flag it warns about can never fire.
pub fn not_armed_report(missing: &[NotArmed]) -> String {
    if missing.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "{TAG} REFUSING TO PRINT: {} constituent channel(s) did not arm. A partial profile is \
         not a smaller profile -- an unarmed channel reads ZERO, and zero is indistinguishable \
         from 'this costs nothing'.\n",
        missing.len(),
    );
    for m in missing {
        out.push_str(&format!(
            "{TAG}   MISSING {:<32} ({}) -- lost: {}\n",
            m.gate,
            m.reason.describe(),
            m.provides,
        ));
    }
    out.push_str(&format!(
        "{TAG}   Re-run with FN64_PROFILE=1, which sets every gate to a value its own parser \
         accepts.\n"
    ));
    out
}

/// Provenance: the command that produced this report.
///
/// Tonight's worst hours went to reconstructing where a number came from. A
/// per-field figure is meaningless without its route and its binary, so the
/// report carries both rather than relying on anyone's memory.
pub struct Provenance {
    pub binary: String,
    pub route: String,
    pub max_steps: String,
    pub warmup_gfx: String,
    /// Which render backend produced these numbers.
    ///
    /// Load-bearing, not decoration: the same route decomposes very differently
    /// on the two backends. The software reference rasterizer puts ~25-26
    /// ms/field in RDP rasterization; RT64 puts ~9.8 ms/field there (measured
    /// 2026-08-14, `entrance-to-match.schedule`, corrected from an earlier
    /// "~4 ms" estimate here that undercounted — see `perf-method.md`'s
    /// "Optimizing RDP rasterization further on the RT64 lane" dead end for
    /// the breakdown). Of that 9.8 ms, only ~1.7 ms (alloc + copy_in +
    /// copy_back) is the fn64-side staging copy; the remaining ~8.0 ms is
    /// time inside RT64's own `processDisplayLists` call -- real GPU
    /// submission/render work, not a fn64-side stall. **A row compared across
    /// backends is not a comparison.** `FN64_RENDER` defaults to `reference`,
    /// so an unset value is a real answer rather than a missing one.
    pub renderer: String,
    /// Each constituent gate with its VERIFIED state, not its declared one.
    pub gates: Vec<(&'static str, String, bool)>,
}

impl Provenance {
    /// Collect from the environment and the verification result.
    pub fn collect(missing: &[NotArmed]) -> Self {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "<unset>".to_string());
        Self {
            binary: std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unknown>".to_string()),
            route: var("FN64_CONTROLLER_SCHEDULE"),
            max_steps: var("FN64_BLOCK_MAX_STEPS"),
            warmup_gfx: var("FN64_FRAME_CENSUS_WARMUP_GFX"),
            // Unset means `reference`, which is the harness default -- report
            // the effective value, not the literal one, so nobody reads
            // "<unset>" as "unknown backend".
            renderer: std::env::var("FN64_RENDER")
                .map(|v| v.to_ascii_lowercase())
                .unwrap_or_else(|_| "reference (FN64_RENDER unset -- harness default)".to_string()),
            gates: CHANNELS
                .iter()
                .map(|c| {
                    let value = var(c.gate);
                    let armed = !missing.iter().any(|m| m.gate == c.gate);
                    (c.gate, value, armed)
                })
                .collect(),
        }
    }

    pub fn report(&self) -> String {
        let mut out = format!(
            "{TAG} PROVENANCE -- a per-field figure without its route and binary is not a \
             result (rule 11a).\n{TAG}   binary:     {}\n{TAG}   route:      {}\n{TAG}   \
             max_steps:  {}\n{TAG}   warmup_gfx: {}\n{TAG}   RENDERER:   {} <-- rows are NOT \
             comparable across backends\n",
            self.binary, self.route, self.max_steps, self.warmup_gfx, self.renderer,
        );
        for (gate, value, armed) in &self.gates {
            out.push_str(&format!(
                "{TAG}   gate {gate:<32} = {value:<8} {}\n",
                if *armed {
                    "ARMED (verified by effect)"
                } else {
                    "NOT ARMED"
                },
            ));
        }
        out
    }
}

/// The instrument's own cost, from an armed/control pair.
///
/// # Why this is measured rather than predicted
///
/// A predicted 0.029 ms/field once measured **+1.62 ms/field — wrong by 56x**.
/// Arming a timer in a hot loop costs what it does to inlining, register
/// pressure and branch layout; the clock read is the cheap part. `FN64_PROFILE`
/// arms six channels at once, so its perturbation is larger than any single
/// gate's and must be stated where the reader cannot miss it.
///
/// `control_ms` comes from a run of the same route with the profile off. When
/// absent the header says the perturbation is UNMEASURED rather than implying
/// it is zero — an unmeasured cost reported as absent is the failure this whole
/// module exists to prevent.
pub fn perturbation_report(armed_ms: f64, control_ms: Option<f64>) -> String {
    let Some(control) = control_ms else {
        return format!(
            "{TAG} PERTURBATION: UNMEASURED. Pass the control lane's ms/field to state it. \
             Arming six channels is not free, and a predicted instrument cost was once wrong \
             by 56x -- do not assume it is small. SHARES below survive the perturbation; \
             ABSOLUTE ms do not.\n"
        );
    };
    let delta = armed_ms - control;
    let ratio = if control > 0.0 {
        armed_ms / control
    } else {
        0.0
    };
    let mut out = format!(
        "{TAG} PERTURBATION: armed {armed_ms:.3} vs control {control:.3} ms/field = \
         {delta:+.3} ms/field ({:.1}%). Correct absolute figures by dividing by {ratio:.4}; \
         SHARES are unaffected.\n",
        100.0 * (ratio - 1.0),
    );
    // A phase smaller than the instrument's own cost is below the instrument's
    // resolution and must not be optimized on this evidence.
    out.push_str(&format!(
        "{TAG}   Any row below {:.3} ms/field is at or under the instrument's own cost and is \
         NOT resolvable by this measurement.\n",
        delta.abs(),
    ));
    if delta.abs() > control * 0.10 {
        out.push_str(&format!(
            "{TAG}   WARNING: the instrument moves the metric by more than 10%. Use the \
             lighter tier (FN64_FRAME_CENSUS + FN64_FRAME_CENSUS_POPULATIONS only) for any \
             absolute claim; this report's shares remain valid.\n"
        ));
    }
    out
}

/// The scope-disambiguation header.
///
/// Six tags carry `gfx_submits` with four different meanings, and a reader
/// comparing a heartbeat's 4820 against a census's 0 needs to know at the point
/// of reading that this is not a contradiction. Stating it in a doc is not
/// enough -- the confusion happens while reading a log.
///
/// # Why the tag names are broken up rather than written literally
///
/// **This function once broke the guest byte-identity checker.** That checker
/// anchors to the LAST line containing `[wm2000-block-progress]`
/// (`check-byte-identity.py`), precisely so a free-text scan cannot pick a
/// different metric of the same name. Printing that tag literally here put a
/// later match in the log, so the anchor landed on THIS EXPLANATORY LINE
/// instead of the data line, and a byte-identical run was reported as
/// `6 not found` -- an instrument breaking another instrument, using the very
/// text that explains name collisions.
///
/// So the tag names are assembled from fragments: the legend still reads
/// correctly to a human, and no anchor an extractor keys on appears as a
/// literal in a non-data line. **A log line is an API when anything parses the
/// log.**
pub fn scope_legend() -> String {
    // Assembled, never literal -- see the note above.
    let progress = format!("[wm2000-block{}progress]", "-");
    let heartbeat = format!("[fn64{}heartbeat]", "-");
    let census = format!("[frame{}census]", "-");
    let sequence = format!("[frame{}sequence]", "-");
    format!(
        "{TAG} NAME SCOPES -- `gfx_submits` appears under six tags with FOUR meanings. This \
         report uses suffixed names so no key here is ambiguous:\n\
         {TAG}   *_run    whole-run cumulative  ({progress}, {heartbeat})\n\
         {TAG}   *_span   steady-state span delta, warmup excluded ({census})\n\
         {TAG}   *_field  per-field delta ({sequence})\n\
         {TAG}   A heartbeat reading 4820 beside a census reading 0 is NOT a contradiction: \
         they are different metrics that share a name.\n"
    )
}

/// Format one row with BOTH denominators.
///
/// A share of the wrong denominator is not a size. Reporting "20.9% of resume
/// NET" is what let three modest-looking rows -- each correct -- be read as
/// small, when they summed to 1.29x the frame budget: the opposite conclusion.
/// Every row therefore states its share of its parent AND its ratio to the
/// 16.667 ms budget.
pub fn row(label: &str, ms: f64, parent_ms: f64, parent_name: &str) -> String {
    let share = if parent_ms > 0.0 {
        100.0 * ms / parent_ms
    } else {
        0.0
    };
    format!(
        "{TAG}   {label:<34} {ms:>8.3}ms/field {share:>6.1}% of {parent_name:<12} \
         {:>5.2}x budget\n",
        ms / FRAME_BUDGET_MS,
    )
}

/// The summed total of a row list, against the denominator THE DECISION uses.
///
/// The rule this enforces: sum the list, do not eye it. Every individual number
/// in the row list can be right while the sum contradicts the conclusion drawn
/// from them.
pub fn total_row(label: &str, values: &[f64]) -> String {
    let total: f64 = values.iter().sum();
    let ratio = total / FRAME_BUDGET_MS;
    format!(
        "{TAG}   {label:<34} {total:>8.3}ms/field  SUM OF THE ROWS ABOVE = {ratio:.2}x the \
         {FRAME_BUDGET_MS:.3}ms budget{}\n",
        if ratio > 1.0 {
            " -- EXCEEDS BUDGET ON ITS OWN: removing everything else still misses 60fps"
        } else {
            ""
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every channel's value must be accepted by its own gate's parser. This is
    /// the test that would have caught "set them all to 1" for the SEQUENCE
    /// gate, which is a count.
    #[test]
    fn sequence_channel_requests_enough_fields_to_show_a_period() {
        let seq = CHANNELS
            .iter()
            .find(|c| c.gate == "FN64_FRAME_CENSUS_SEQUENCE")
            .expect("sequence channel present");
        let n: usize = seq.value.parse().expect("sequence value must be numeric");
        assert!(
            n >= 100,
            "a sequence dump of {n} cannot show a period; 1 field is the naive `=1` bug",
        );
    }

    /// No channel may be armed with `0`. Under `is_some()` gates that ARMS, and
    /// under `env_flag` gates it disarms -- so `0` means opposite things and is
    /// never a safe value to write.
    #[test]
    fn no_channel_is_armed_with_zero() {
        for c in CHANNELS {
            assert_ne!(
                c.value, "0",
                "{}: `0` arms is_some() gates and disarms env_flag gates",
                c.gate,
            );
        }
    }

    /// EVERY GATE NAMED IN THE COUNTER TREE MUST BE COMPOSED.
    ///
    /// Closes the hole that produced three silent 0.000 staging rows on the
    /// first real run: `FN64_DPC_COPY_CENSUS` was declared in the tree but
    /// absent from `CHANNELS`, so its counters were never armed and read as a
    /// clean zero with no warning -- the exact "unarmed reads as zero" failure
    /// this module exists to prevent.
    ///
    /// Enumerating channels by hand is the same class of error as enumerating
    /// nesting by hand. This makes adding a counter to the tree without
    /// composing its gate a TEST FAILURE rather than a silent zero in a row
    /// somebody is about to optimize.
    #[test]
    fn every_gate_the_tree_declares_is_composed() {
        for node in crate::counter_tree::TREE {
            assert!(
                CHANNELS.iter().any(|c| c.gate == node.gate),
                "counter `{}` declares gate {} but FN64_PROFILE does not arm it -- its rows \
                 would read 0.000 with no warning",
                node.name,
                node.gate,
            );
        }
    }

    /// All five gates the brief names must be composed, plus the census itself.
    #[test]
    fn composes_every_gate_a_decomposition_needs() {
        for required in [
            "FN64_PHASE_TIMING",
            "FN64_EXECUTOR_SPLIT",
            "FN64_FRAME_CENSUS_POPULATIONS",
            "FN64_RESUME_SPLIT",
            "FN64_FRAME_CENSUS_SEQUENCE",
            "FN64_FRAME_CENSUS",
        ] {
            assert!(
                CHANNELS.iter().any(|c| c.gate == required),
                "{required} must be composed by FN64_PROFILE",
            );
        }
    }

    /// THE CHECK: a channel that is set but produced nothing must be reported
    /// as missing. This is the `=0`-arms-an-is_some()-gate case -- the variable
    /// is present, an echo would call it armed, and it produced no data.
    #[test]
    fn a_set_but_dead_channel_is_reported_missing() {
        let witness = |_gate: &str| false;
        let missing = verify(&witness);
        assert_eq!(
            missing.len(),
            CHANNELS.len(),
            "no channel produced data, so all must be reported",
        );
        let text = not_armed_report(&missing);
        assert!(text.contains("REFUSING TO PRINT"));
        for c in CHANNELS {
            assert!(text.contains(c.gate), "notice must name {}", c.gate);
        }
    }

    /// A fully-armed run must produce NO missing report, or the check fires
    /// always and means nothing (rule 6a).
    #[test]
    fn a_fully_armed_run_reports_nothing_missing() {
        let witness = |_gate: &str| true;
        assert!(verify(&witness).is_empty());
        assert!(not_armed_report(&[]).is_empty());
    }

    /// Exactly the named gate is reported, not a generic failure.
    #[test]
    fn the_notice_names_the_specific_missing_gate() {
        let witness = |gate: &str| gate != "FN64_RESUME_SPLIT";
        let missing = verify(&witness);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].gate, "FN64_RESUME_SPLIT");
        let text = not_armed_report(&missing);
        assert!(text.contains("FN64_RESUME_SPLIT"));
        assert!(
            text.contains("resume NET decomposition"),
            "must say what was lost, not just that something was",
        );
    }

    /// The NOT-ARMED notice must be reachable with `FN64_PROFILE` absent. A
    /// warning behind the flag it warns about is unreachable by construction --
    /// the trap that cost two 25-minute runs.
    #[test]
    fn the_not_armed_notice_does_not_depend_on_the_profile_flag() {
        let missing = vec![NotArmed {
            gate: "FN64_EXECUTOR_SPLIT",
            provides: "the executor split",
            reason: Reason::Unset,
        }];
        // `not_armed_report` consults no gate: it is a pure function of its
        // argument, so no flag can silence it.
        let text = not_armed_report(&missing);
        assert!(!text.is_empty());
        assert!(text.contains("FN64_EXECUTOR_SPLIT"));
    }

    /// BOTH denominators on every row -- the fix for the single most
    /// consequential error: "20.9% of resume NET" hid that the row was 0.57x
    /// the budget.
    #[test]
    fn every_row_states_both_denominators() {
        let line = row(
            "dispatch = TRANSLATED GUEST CODE",
            9.528,
            45.687,
            "resume NET",
        );
        assert!(line.contains("20.9%"), "share of parent: {line}");
        assert!(line.contains("0.57x budget"), "ratio to budget: {line}");
    }

    /// THE DEFECT, replayed: three rows that each look modest against their
    /// parent but total 1.29x the budget. Every individual number was right and
    /// the sum was never computed.
    #[test]
    fn the_sum_of_modest_rows_exposes_a_budget_breach() {
        // 9.528 + 8.848 + 2.31 + ... as recorded: the non-graphics rows.
        let rows = [9.528, 8.848, 2.31, 0.574, 0.29];
        let line = total_row("HOST-SIDE TOTAL", &rows);
        assert!(line.contains("1.29x"), "must compute the sum: {line}");
        assert!(
            line.contains("EXCEEDS BUDGET"),
            "a sum over 1.0x must say so outright: {line}",
        );
    }

    /// THE RT64 CASE, which is the harder one and the lane the owner runs.
    ///
    /// Six rows, NONE above 0.59x the budget, totalling 1.94x. On the reference
    /// backend one row (RDP at 26.4 ms) is visibly enormous and any format
    /// would flag it. Here every row looks individually modest and the program
    /// still misses 60fps by nearly 2x -- so the SUM is the only thing that can
    /// show it. This is precisely the failure the format exists to prevent.
    #[test]
    fn six_individually_modest_rt64_rows_are_shown_to_total_almost_2x() {
        let rows = [
            ("guest code", 9.79),
            ("mirror", 9.01),
            ("RSP", 5.76),
            ("rasterization", 4.00),
            ("invalidate", 2.04),
            ("staging memcpy", 1.77),
        ];
        // Not one row would raise an eyebrow on its own.
        for (label, ms) in rows {
            let line = row(label, ms, 32.37, "field");
            let ratio = ms / FRAME_BUDGET_MS;
            assert!(
                ratio < 0.6,
                "{label} is {ratio:.2}x budget -- this test's premise is that no single row \
                 looks alarming",
            );
            assert!(line.contains("x budget"), "{line}");
        }
        // The sum is what indicts the program.
        let line = total_row("FIELD TOTAL", &rows.map(|(_, v)| v));
        assert!(line.contains("1.94x"), "must total 1.94x: {line}");
        assert!(
            line.contains("EXCEEDS BUDGET"),
            "1.94x must be called out explicitly: {line}",
        );
    }

    /// An unmeasured perturbation must SAY unmeasured, never imply zero.
    #[test]
    fn an_unmeasured_perturbation_is_reported_as_unmeasured() {
        let text = perturbation_report(45.0, None);
        assert!(text.contains("UNMEASURED"), "{text}");
        assert!(
            text.contains("56x"),
            "must carry why prediction is not allowed: {text}",
        );
    }

    /// A measured pair states the delta, the correction factor, and the
    /// resolution floor below which a row cannot be trusted.
    #[test]
    fn a_measured_perturbation_states_the_resolution_floor() {
        let text = perturbation_report(46.62, Some(45.00));
        assert!(text.contains("+1.620 ms/field"), "{text}");
        assert!(
            text.contains("NOT resolvable"),
            "a phase under the instrument's own cost must be called unresolvable: {text}",
        );
    }

    /// A large perturbation must warn and offer the lighter tier.
    #[test]
    fn a_large_perturbation_warns_and_offers_a_lighter_tier() {
        let text = perturbation_report(60.0, Some(45.0));
        assert!(text.contains("WARNING"), "{text}");
        assert!(text.contains("lighter tier"), "{text}");
    }

    /// A small perturbation must NOT warn, or the warning is noise.
    #[test]
    fn a_small_perturbation_does_not_warn() {
        let text = perturbation_report(45.2, Some(45.0));
        assert!(!text.contains("WARNING"), "{text}");
    }

    /// A total under budget must NOT claim a breach, or the warning is noise.
    #[test]
    fn a_total_under_budget_does_not_warn() {
        let line = total_row("HOST-SIDE TOTAL", &[1.0, 2.0]);
        assert!(!line.contains("EXCEEDS BUDGET"));
        assert!(line.contains("0.18x"));
    }

    /// The scope legend must state the collision explicitly, at the point of
    /// reading.
    #[test]
    fn the_scope_legend_disambiguates_the_colliding_name() {
        let legend = scope_legend();
        assert!(legend.contains("gfx_submits"));
        assert!(legend.contains("_run"));
        assert!(legend.contains("_span"));
        assert!(legend.contains("_field"));
        assert!(
            legend.contains("NOT a contradiction"),
            "the reader needs the resolution, not just the warning",
        );
    }

    /// NO REPORT LINE MAY BEGIN WITH AN ANCHOR TAG ANOTHER EXTRACTOR KEYS ON.
    ///
    /// This regression exists because the legend above once broke the guest
    /// byte-identity checker: it cited `[wm2000-block-progress]` as an example
    /// of the collision it explains, `check-byte-identity.py` anchors to the
    /// LAST line containing that tag, and the explanation outranked the data
    /// line. A byte-identical run reported `6 not found`.
    ///
    /// Every line this module emits must start with `[fn64-profile]`, so a
    /// substring anchor on any other tag cannot select one. A log line is an
    /// API when anything parses the log.
    #[test]
    fn no_emitted_line_starts_with_another_tools_anchor_tag() {
        let mut emitted = String::new();
        emitted.push_str(&scope_legend());
        emitted.push_str(&perturbation_report(45.0, Some(44.0)));
        emitted.push_str(&not_armed_report(&[NotArmed {
            gate: "FN64_RESUME_SPLIT",
            provides: "the resume split",
            reason: Reason::Unset,
        }]));
        emitted.push_str(&row("some row", 1.0, 2.0, "parent"));
        emitted.push_str(&total_row("total", &[1.0]));
        for line in emitted.lines().filter(|l| !l.trim().is_empty()) {
            assert!(
                line.starts_with(TAG),
                "every profile line must start with {TAG} so another tool's \
                 anchor cannot select it; got: {line}",
            );
        }
    }

    /// Provenance must carry the binary and route, and report each gate's
    /// VERIFIED state rather than its declared one.
    #[test]
    fn provenance_reports_verified_state_not_declared_state() {
        let missing = vec![NotArmed {
            gate: "FN64_RESUME_SPLIT",
            provides: "the resume split",
            reason: Reason::Unset,
        }];
        let p = Provenance::collect(&missing);
        let text = p.report();
        assert!(text.contains("PROVENANCE"));
        assert!(text.contains("binary:"));
        assert!(text.contains("route:"));
        // The one that failed is marked NOT ARMED even though the others pass.
        let resume_line = text
            .lines()
            .find(|l| l.contains("FN64_RESUME_SPLIT"))
            .expect("resume gate listed");
        assert!(resume_line.contains("NOT ARMED"), "{resume_line}");
        let census_line = text
            .lines()
            .find(|l| l.contains("FN64_FRAME_CENSUS "))
            .expect("census gate listed");
        assert!(census_line.contains("ARMED"), "{census_line}");
    }

    /// `FN64_PROFILE` uses the strict convention: `0` is off. The new gate must
    /// not inherit the `is_some()` trap it exists to paper over.
    #[test]
    fn the_profile_gate_treats_zero_as_off() {
        // `enabled()` memoizes, so exercise the predicate shape directly.
        let parse = |v: &str| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        };
        assert!(parse("1"));
        assert!(parse("on"));
        assert!(!parse("0"), "0 must be OFF for the new gate");
        assert!(
            !parse(""),
            "empty must be OFF -- an empty value reading as ON fabricated a 4.9x"
        );
    }
}
