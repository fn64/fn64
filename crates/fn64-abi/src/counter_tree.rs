//! The counter tree, declared once as DATA rather than re-derived by hand at
//! every report site.
//!
//! # Why this exists
//!
//! The nesting relationships between the phase counters are correct, detailed,
//! and were already written down -- as an ASCII diagram in a comment on
//! [`crate::task_dispatch::lifecycle::PhaseTiming`]. That diagram is right. The
//! problem is that it is prose: every report site re-implemented the nesting as
//! hand-written subtraction, and **three of them got it wrong in a single
//! evening**:
//!
//! - a phase clock lapped across a coroutine suspend -- closure residual
//!   **-697%**;
//! - the partial fix, covering one suspend site of sixteen -- **-533%**;
//! - a bucket labelled `resolve next entry` that was **95% graphics**, caught
//!   only because a human noticed a child (`gfx_ns` 21.5) exceeding its parent
//!   (7.7).
//!
//! None was visible in the phase values; all three looked like findings. **A
//! diagram a human must obey is not a check.** This module promotes the diagram
//! into a table the code checks, so the class of error is gone rather than
//! merely documented.
//!
//! # The relationship that was actually confusing
//!
//! `exec_mirror_ns` is nested INSIDE `exec_resume_ns`, and is simultaneously a
//! SIBLING of `resume NET`. Both are true: `resume NET` is *defined* as
//! `exec_resume_ns - exec_mirror_ns - exec_guard_suspend_ns`, so the mirror is
//! inside the one and disjoint from the other. Prose made those two facts
//! confusable and that is exactly what the hand-written subtractions got wrong.
//! [`Node::Derived`] states both without ambiguity.
//!
//! # What the check is
//!
//! For every node, the sum of its children must not exceed it. A violation
//! **refuses to print the affected subtree** -- it is a hard error, not a
//! warning, because the three defects above were all "visible" in the sense
//! that the numbers were on screen, and all three were believed anyway.
//!
//! Tolerance is proportional, not absolute: these are wall-clock sums over
//! thousands of fields, so exact equality is not achievable and a fixed epsilon
//! would be wrong at both ends of the scale.

/// Fractional slack allowed before children are said to exceed their parent.
///
/// Not zero: each counter is an independent `Instant::now` pair, so a child can
/// legitimately exceed its parent by a few clock reads' worth of skew. Not
/// large either -- the defects this exists to catch were -697%, -533%, and a
/// child at 279% of its parent. Anything in between is a real violation.
///
/// 2% is chosen so that the smallest historical defect (a child at 2.79x its
/// parent) is caught with three orders of magnitude of margin, while ordinary
/// clock skew across ~7,700 fields is not.
pub const CLOSURE_TOLERANCE: f64 = 0.02;

/// One counter's place in the tree.
///
/// `Measured` nodes come straight from a timer. `Derived` nodes are computed by
/// subtracting nested counters from a measured parent -- the operation every
/// report site previously open-coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Read directly from a phase timer.
    Measured,
    /// `base` minus each of `subtract`. This is how `resume NET` is defined,
    /// and stating it here is what makes "inside `exec_resume_ns`, sibling of
    /// `resume NET`" unambiguous.
    Derived {
        base: &'static str,
        subtract: &'static [&'static str],
    },
}

/// A node in the counter tree: what it is called, what contains it, and whether
/// it is measured or derived.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    /// The counter's name, matching [`super::frame_census`]'s labels.
    pub name: &'static str,
    /// The counter that CONTAINS this one, if any. `None` marks a root.
    pub parent: Option<&'static str>,
    pub kind: Kind,
    /// Which env gate must be armed for this counter to carry data. A zero
    /// reading means "not armed", not "costs nothing" -- the distinction that
    /// the NOT-ARMED notices exist to preserve.
    pub gate: &'static str,
}

const fn measured(name: &'static str, parent: Option<&'static str>, gate: &'static str) -> Node {
    Node { name, parent, kind: Kind::Measured, gate }
}

/// The tree, transcribed from the `PhaseTiming` diagram at
/// `task_dispatch/lifecycle.rs`. That comment is now a pointer to this table
/// rather than the source of truth.
///
/// Order is parent-before-child so a reader can follow the nesting top to
/// bottom; [`validate`] does not depend on the order.
pub const TREE: &[Node] = &[
    // ---- root: the whole host step.
    measured("executor_ns", None, "FN64_PHASE_TIMING"),
    // ---- executor split. mirror and guard@suspend are INSIDE resume;
    //      guard@device is INSIDE devtime.
    measured("exec_resume_ns", Some("executor_ns"), "FN64_EXECUTOR_SPLIT"),
    measured("exec_mirror_ns", Some("exec_resume_ns"), "FN64_EXECUTOR_SPLIT"),
    measured(
        "exec_guard_suspend_ns",
        Some("exec_resume_ns"),
        "FN64_EXECUTOR_SPLIT",
    ),
    measured("exec_devtime_ns", Some("executor_ns"), "FN64_EXECUTOR_SPLIT"),
    measured(
        "exec_guard_device_ns",
        Some("exec_devtime_ns"),
        "FN64_EXECUTOR_SPLIT",
    ),
    // ---- `resume NET`: guest + runtime, with the apparatus nested inside the
    //      resume taken back out. THE relationship that prose made confusable.
    Node {
        name: "resume_net",
        parent: Some("exec_resume_ns"),
        kind: Kind::Derived {
            base: "exec_resume_ns",
            subtract: &["exec_mirror_ns", "exec_guard_suspend_ns"],
        },
        gate: "FN64_EXECUTOR_SPLIT",
    },
    // ---- resume split. Children of `resume_net`, NOT of `exec_resume_ns`:
    //      the phases exclude the mirror and the suspend-guard by construction.
    measured("resume_reconcile_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_cop0_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_dispatch_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_invalidate_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_exit_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_suspend_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_resolve_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    measured("resume_hostcall_ns", Some("resume_net"), "FN64_RESUME_SPLIT"),
    // ---- graphics and audio are reached through the OS-call shims, so they
    //      nest inside `resume_hostcall_ns`. Reading them as peers of it is the
    //      error that hid 21.72 ms one level up.
    measured("gfx_ns", Some("resume_hostcall_ns"), "FN64_PHASE_TIMING"),
    measured("gfx_lle_ns", Some("gfx_ns"), "FN64_PHASE_TIMING"),
    measured("gfx_lle_rsp_ns", Some("gfx_lle_ns"), "FN64_PHASE_TIMING"),
    measured("gfx_lle_rdp_ns", Some("gfx_lle_ns"), "FN64_PHASE_TIMING"),
    // ---- the staging copy, nested under the RDP seam that performs it.
    //      `dispatch_captured_raw_rdp` stages a whole-RDRAM image per call;
    //      these three name where that time goes. Declared as children of the
    //      RDP node so the tree checks them against it rather than letting a
    //      staging cost be read as rasterization.
    measured("dpc_alloc_ns", Some("gfx_lle_rdp_ns"), "FN64_DPC_COPY_CENSUS"),
    measured("dpc_copy_in_ns", Some("gfx_lle_rdp_ns"), "FN64_DPC_COPY_CENSUS"),
    measured(
        "dpc_copy_back_ns",
        Some("gfx_lle_rdp_ns"),
        "FN64_DPC_COPY_CENSUS",
    ),
    measured("audio_lle_ns", Some("resume_hostcall_ns"), "FN64_PHASE_TIMING"),
    // ---- NOT in the tree above: presentation runs on the harness's
    //      `advance_virtual_time` arm, OUTSIDE `executor_ns`. It is a root of
    //      its own, and `vi_present_in_executor_calls` exists to prove that by
    //      observation rather than by argument. Parenting it under
    //      `executor_ns` would be the very inference this table forbids.
    measured("vi_present_ns", None, "FN64_PHASE_TIMING"),
];

/// A parent whose children claim more time than it contains.
///
/// Carries the numbers that prove it, because "the instrument is broken" is
/// only actionable with the arithmetic attached.
#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    pub parent: &'static str,
    pub parent_ms: f64,
    pub children_ms: f64,
    /// Per-child contributions, largest first: the row that broke it is
    /// usually the one to look at.
    pub children: Vec<(&'static str, f64)>,
}

impl Violation {
    /// How far past its parent the subtree claims to reach.
    pub fn overshoot_ratio(&self) -> f64 {
        if self.parent_ms > 0.0 {
            self.children_ms / self.parent_ms
        } else {
            f64::INFINITY
        }
    }

    /// The refusal notice. Deliberately blunt: the three defects this exists to
    /// catch were all on screen and all believed anyway.
    pub fn report(&self) -> String {
        let mut out = format!(
            "[fn64-profile] TREE VIOLATION under {}: children sum to {:.3}ms but the parent is \
             {:.3}ms ({:.1}x). REFUSING to print this subtree -- the instrument is broken, and \
             the rows it would contain are not measurements.\n",
            self.parent,
            self.children_ms,
            self.parent_ms,
            self.overshoot_ratio(),
        );
        for (name, ms) in &self.children {
            out.push_str(&format!(
                "[fn64-profile]   child {name:<28} {ms:>10.3}ms\n"
            ));
        }
        out.push_str(
            "[fn64-profile]   A child exceeding its parent means a timer spans work it does not \
             own -- historically a clock lapped across a coroutine suspend, or a bucket carrying \
             another phase's cost under a plausible name. Fix the instrument before reading any \
             number from this run.\n",
        );
        out
    }
}

/// Look a node up by name.
pub fn node(name: &str) -> Option<&'static Node> {
    TREE.iter().find(|n| n.name == name)
}

/// Every node that declares `parent` as its container.
///
/// Derived nodes are excluded from their parent's child set: `resume_net` is
/// `exec_resume_ns` minus two of its children, so counting it alongside them
/// would double-count the remainder and manufacture a violation.
pub fn children_of(parent: &str) -> Vec<&'static Node> {
    TREE.iter()
        .filter(|n| n.parent == Some(parent) && !matches!(n.kind, Kind::Derived { .. }))
        .collect()
}

/// Resolve a node's value, computing derived nodes from their base.
///
/// `lookup` supplies measured values in milliseconds; a counter whose gate is
/// unarmed reads zero, and the caller is responsible for distinguishing that
/// from a genuine zero.
pub fn value_of(name: &str, lookup: &dyn Fn(&str) -> f64) -> f64 {
    match node(name).map(|n| n.kind) {
        Some(Kind::Derived { base, subtract }) => {
            let base_ms = value_of(base, lookup);
            let taken: f64 = subtract.iter().map(|s| value_of(s, lookup)).sum();
            (base_ms - taken).max(0.0)
        }
        _ => lookup(name),
    }
}

/// Check every parent in the tree against the sum of its children.
///
/// Returns one [`Violation`] per offending parent. An empty vector means the
/// tree closed and the rows may be printed.
///
/// Nodes whose gate is unarmed read zero and cannot trip the check -- a zero
/// child never exceeds its parent -- so an unarmed run is silent here rather
/// than noisy, and the NOT-ARMED notices carry that message instead.
pub fn validate(lookup: &dyn Fn(&str) -> f64) -> Vec<Violation> {
    let mut violations = Vec::new();
    for parent in TREE {
        let kids = children_of(parent.name);
        if kids.is_empty() {
            continue;
        }
        let parent_ms = value_of(parent.name, lookup);
        // An unmeasured parent (gate off) has unmeasured children too; skip
        // rather than report a violation that is really an arming problem.
        if parent_ms <= 0.0 {
            continue;
        }
        let mut children: Vec<(&'static str, f64)> = kids
            .iter()
            .map(|k| (k.name, value_of(k.name, lookup)))
            .collect();
        let children_ms: f64 = children.iter().map(|(_, v)| v).sum();
        if children_ms > parent_ms * (1.0 + CLOSURE_TOLERANCE) {
            children.sort_by(|a, b| b.1.total_cmp(&a.1));
            violations.push(Violation {
                parent: parent.name,
                parent_ms,
                children_ms,
                children,
            });
        }
    }
    violations
}

/// Whether any violation names this node or an ancestor of it, i.e. whether the
/// node sits in a subtree the report must refuse to print.
pub fn suppressed_by(name: &str, violations: &[Violation]) -> bool {
    let mut cursor = Some(name);
    while let Some(current) = cursor {
        if violations.iter().any(|v| v.parent == current) {
            return true;
        }
        cursor = node(current).and_then(|n| n.parent);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a lookup over a name->ms map, defaulting to zero.
    fn lookup_from(pairs: &[(&'static str, f64)]) -> impl Fn(&str) -> f64 {
        let map: HashMap<&'static str, f64> = pairs.iter().copied().collect();
        move |name: &str| map.get(name).copied().unwrap_or(0.0)
    }

    /// The tree must be internally consistent: every declared parent and every
    /// `Derived` reference must name a node that exists. A typo here would
    /// silently drop a counter out of the check, which is the failure mode the
    /// module exists to prevent.
    #[test]
    fn every_reference_resolves() {
        for n in TREE {
            if let Some(parent) = n.parent {
                assert!(node(parent).is_some(), "{}: unknown parent {parent}", n.name);
            }
            if let Kind::Derived { base, subtract } = n.kind {
                assert!(node(base).is_some(), "{}: unknown base {base}", n.name);
                for s in subtract {
                    assert!(node(s).is_some(), "{}: unknown subtrahend {s}", n.name);
                }
            }
        }
    }

    #[test]
    fn no_duplicate_names() {
        let mut seen = std::collections::HashSet::new();
        for n in TREE {
            assert!(seen.insert(n.name), "duplicate node {}", n.name);
        }
    }

    /// THE relationship that prose made confusable, asserted as an invariant:
    /// the mirror is INSIDE `exec_resume_ns` and DISJOINT from `resume NET`.
    /// Three report sites got this wrong by hand.
    #[test]
    fn mirror_is_inside_resume_but_sibling_of_resume_net() {
        let mirror = node("exec_mirror_ns").expect("mirror in tree");
        assert_eq!(
            mirror.parent,
            Some("exec_resume_ns"),
            "the mirror is nested inside the resume",
        );
        let net = node("resume_net").expect("resume_net in tree");
        let Kind::Derived { base, subtract } = net.kind else {
            panic!("resume_net must be derived, not measured");
        };
        assert_eq!(base, "exec_resume_ns");
        assert!(
            subtract.contains(&"exec_mirror_ns"),
            "resume NET must subtract the mirror -- that is what makes them siblings",
        );
        // And therefore the mirror is NOT among resume_net's children.
        assert!(
            !children_of("resume_net").iter().any(|c| c.name == "exec_mirror_ns"),
            "the mirror must not be counted inside resume NET as well",
        );
    }

    /// A derived node must not be counted as a child of its own base, or the
    /// remainder is double-counted and the check invents a violation.
    #[test]
    fn derived_nodes_are_not_children_of_their_base() {
        assert!(
            !children_of("exec_resume_ns").iter().any(|c| c.name == "resume_net"),
            "resume_net is derived FROM exec_resume_ns, not an additional child of it",
        );
    }

    /// A healthy decomposition must pass. Without this the violation tests
    /// below prove nothing -- a check that always fires is as useless as one
    /// that never does (rule 6a).
    #[test]
    fn a_closing_tree_reports_no_violation() {
        let lookup = lookup_from(&[
            ("executor_ns", 60.0),
            ("exec_resume_ns", 55.0),
            ("exec_mirror_ns", 8.848),
            ("exec_guard_suspend_ns", 0.5),
            ("exec_devtime_ns", 3.0),
            ("exec_guard_device_ns", 1.0),
            ("resume_reconcile_ns", 1.0),
            ("resume_cop0_ns", 1.0),
            ("resume_dispatch_ns", 9.528),
            ("resume_invalidate_ns", 0.5),
            ("resume_exit_ns", 0.5),
            ("resume_suspend_ns", 0.0),
            ("resume_resolve_ns", 0.5),
            ("resume_hostcall_ns", 33.0),
            ("gfx_ns", 32.119),
            ("gfx_lle_ns", 32.0),
            ("gfx_lle_rsp_ns", 5.637),
            ("gfx_lle_rdp_ns", 26.396),
            ("audio_lle_ns", 0.5),
        ]);
        let violations = validate(&lookup);
        assert!(
            violations.is_empty(),
            "healthy tree must not fire: {violations:?}",
        );
    }

    /// THE DEFECT, replayed: `gfx_ns` at 21.5 under a parent of 7.7. This was
    /// caught tonight only because a human noticed the child exceeded its
    /// parent. Now it is mechanical.
    #[test]
    fn child_exceeding_parent_is_caught_and_subtree_refused() {
        let lookup = lookup_from(&[
            ("executor_ns", 40.0),
            ("exec_resume_ns", 38.0),
            ("resume_hostcall_ns", 7.7),
            ("gfx_ns", 21.5),
        ]);
        let violations = validate(&lookup);
        let hostcall = violations
            .iter()
            .find(|v| v.parent == "resume_hostcall_ns")
            .expect("a child at 279% of its parent must be caught");
        assert!((hostcall.parent_ms - 7.7).abs() < 1e-9);
        assert!((hostcall.children_ms - 21.5).abs() < 1e-9);
        assert!(hostcall.overshoot_ratio() > 2.7);
        // The whole subtree beneath the violation is refused, not just the row.
        assert!(suppressed_by("gfx_lle_rdp_ns", &violations));
        assert!(suppressed_by("gfx_ns", &violations));
        // And the notice names the offender with its arithmetic.
        let text = hostcall.report();
        assert!(text.contains("TREE VIOLATION"));
        assert!(text.contains("REFUSING"));
        assert!(text.contains("gfx_ns"));
    }

    /// The `-697%` shape: phases claiming far more than the parent that
    /// contains them, which is what a clock lapped across a coroutine suspend
    /// produces.
    #[test]
    fn phases_overshooting_resume_net_are_caught() {
        // resume NET = 55 - 8 - 0 = 47, but `resolve` alone claims 197.
        let lookup = lookup_from(&[
            ("executor_ns", 60.0),
            ("exec_resume_ns", 55.0),
            ("exec_mirror_ns", 8.0),
            ("resume_resolve_ns", 197.0),
        ]);
        let violations = validate(&lookup);
        let net = violations
            .iter()
            .find(|v| v.parent == "resume_net")
            .expect("a 197ms phase inside a 47ms parent must be caught");
        assert!(net.overshoot_ratio() > 4.0);
        assert_eq!(net.children.first().map(|(n, _)| *n), Some("resume_resolve_ns"));
    }

    /// A subtree under no violation must stay printable -- the check refuses
    /// the affected subtree, not the whole report.
    #[test]
    fn unaffected_subtrees_are_not_suppressed() {
        let lookup = lookup_from(&[
            ("executor_ns", 40.0),
            ("exec_resume_ns", 38.0),
            ("resume_hostcall_ns", 7.7),
            ("gfx_ns", 21.5),
            ("exec_devtime_ns", 1.0),
            ("exec_guard_device_ns", 0.5),
        ]);
        let violations = validate(&lookup);
        assert!(!violations.is_empty());
        assert!(
            !suppressed_by("exec_guard_device_ns", &violations),
            "devtime is a different subtree and must remain readable",
        );
    }

    /// An unarmed gate reads zero everywhere and must not manufacture a
    /// violation: that would report an arming problem as an instrument defect.
    #[test]
    fn an_unarmed_tree_is_silent() {
        let lookup = lookup_from(&[]);
        assert!(validate(&lookup).is_empty());
    }

    /// Clock skew of a few microseconds across thousands of fields must not
    /// fire, or the check becomes noise and gets ignored.
    #[test]
    fn tolerance_absorbs_clock_skew_but_not_a_real_defect() {
        let near = lookup_from(&[
            ("executor_ns", 100.0),
            ("exec_resume_ns", 100.5),
        ]);
        assert!(
            validate(&near).is_empty(),
            "0.5% over parent is clock skew, not a defect",
        );
        let real = lookup_from(&[
            ("executor_ns", 100.0),
            ("exec_resume_ns", 110.0),
        ]);
        assert!(
            !validate(&real).is_empty(),
            "10% over parent is a real violation",
        );
    }

    /// `resume NET` must equal base minus the nested apparatus -- the
    /// subtraction three report sites open-coded.
    #[test]
    fn resume_net_is_computed_not_looked_up() {
        let lookup = lookup_from(&[
            ("exec_resume_ns", 55.0),
            ("exec_mirror_ns", 8.848),
            ("exec_guard_suspend_ns", 0.465),
        ]);
        let net = value_of("resume_net", &lookup);
        assert!((net - (55.0 - 8.848 - 0.465)).abs() < 1e-9, "got {net}");
    }

    /// Presentation is NOT inside the executor. Parenting it there would be an
    /// inference; the reachability counters exist to settle it by observation.
    #[test]
    fn vi_present_is_a_root_not_an_executor_child() {
        let vi = node("vi_present_ns").expect("vi_present_ns in tree");
        assert_eq!(vi.parent, None, "presentation runs outside executor_ns");
    }
}
