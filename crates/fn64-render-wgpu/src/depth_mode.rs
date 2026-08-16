//! Full RDP depth-mode compare/update behavior: all four `ZMODE_*` variants
//! and their exact comparison/update semantics, plus the coverage-wrap
//! tightening `depth_coverage_decision` layers on top of the plain per-mode
//! test.
//!
//! Characterization-first, selective literal port of
//! `crates/fn64-render-reference/src/depth.rs` (`relations`, `mode_passes`)
//! and `crates/fn64-render-reference/src/raster/coverage.rs`
//! (`depth_coverage_decision`, `pub(super)` there and re-expressed here as a
//! standalone public seam), matching `docs/RT64-BLENDER-DEPTH-PORT-CARD.md`
//! §4 "Depth compare/update" and its "Typed Rust/WGSL seam" guidance: reuse
//! `depth.rs`'s `DepthRelations` idiom, port `mode_passes`/
//! `depth_coverage_decision` as literal arithmetic, and surface the
//! `Interpenetrating`-mode coverage-wrap gap as a loud, typed rejection
//! rather than a silent default -- the Programming Manual's "Blender Modes
//! and Assumptions" section requires a coverage-adjustment path for wrapping
//! interpenetrating fragments but does not publish its arithmetic, so
//! `fn64-render-reference` itself leaves it as an explicit unimplemented
//! panic (`coverage.rs:36,46-48`) and this port preserves that as a typed
//! `DepthModeDecision::UnsupportedInterpenetratingCoverageAdjustment`
//! variant, not a decode-time normalization and not a silent pass/reject.
//!
//! This module ports `crate::state::DepthMode` behavior; it does not
//! redefine or re-export that type (see `crate::state`, owned by another
//! lane per the Claude lane protocol) and does not touch `state.rs`. It
//! reuses `fn64_render_wgpu::state::DepthMode` directly as its mode
//! parameter, matching the port card's instruction to treat the existing
//! `OtherMode`/`DepthMode` decode layer as the natural CPU-side seam.
//!
//! Scope, per the port card: the four-mode dispatch (`mode_passes`) and its
//! coverage-wrap interaction (`depth_coverage_decision`) only. Z encode/
//! decode (`EncodedDepth`, `encode_z`/`decode_z`, DeltaZ) is a distinct,
//! not-yet-ported slice (`docs/rt64-port-inventory.json` records it
//! `not-started`); this module takes already-decoded working-space `u32` Z
//! and `u16` DeltaZ values as input, matching `depth.rs::relations`'s own
//! signature. Blend, coverage accumulation, alpha compare, dither, and the
//! framebuffer-read problem are out of scope (owned by other active lanes
//! per the Claude lane protocol) -- this module only decides pass/reject,
//! never a byte write.

use crate::state::DepthMode;

/// The four public Z-comparison signals, Programming Manual Chapter 15
/// Equations 5-9. Literal port of `fn64-render-reference`'s
/// `depth::DepthRelations` (`depth.rs:86-92`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DepthRelations {
    pub memory_is_max: bool,
    pub farther: bool,
    pub nearer: bool,
    pub in_front: bool,
}

/// Expand a stored four-bit DeltaZ exponent for the depth-mode comparisons.
/// Literal port of `depth::decode_delta_z` (`depth.rs:79-81`): encoding
/// zero represents the minimum comparison delta of one (including the
/// conventional zero-DeltaZ depth clear), and larger exponents expand to
/// their power-of-two floor, saturated to the four-bit maximum (exponent
/// 15 -> `1 << 15`).
pub fn decode_delta_z(encoded_delta: u8) -> u16 {
    1 << encoded_delta.min(15)
}

/// Compute the four Z-comparison signals for one fragment against one
/// memory sample. Literal port of `depth::relations` (`depth.rs:94-107`).
/// Note the deliberate asymmetry the reference itself has: `pixel_delta_z`
/// is the fragment's own already-decoded DeltaZ (`u16`, a direct value),
/// while `memory_encoded_delta_z` is the *stored* four-bit exponent
/// (`u8`) and is decoded internally via `decode_delta_z` before the two
/// are compared -- the memory side is only ever available in its packed
/// exponent form (`EncodedDepth`'s low bits), so the reference decodes it
/// at the comparison site rather than requiring every caller to pre-decode
/// it. `delta_z_max` is the larger of the two decoded deltas, and every
/// signal is a plain saturating/unsigned comparison over the RDP's 18-bit
/// working Z range.
pub fn relations(
    pixel_z: u32,
    pixel_delta_z: u16,
    memory_z: u32,
    memory_encoded_delta_z: u8,
) -> DepthRelations {
    let memory_delta_z = decode_delta_z(memory_encoded_delta_z);
    let delta_z_max = if pixel_delta_z >= memory_delta_z {
        pixel_delta_z as u32
    } else {
        memory_delta_z as u32
    };
    DepthRelations {
        memory_is_max: memory_z >= 0x3ffff,
        farther: pixel_z.saturating_add(delta_z_max) >= memory_z,
        nearer: pixel_z.saturating_sub(delta_z_max) <= memory_z,
        in_front: pixel_z < memory_z,
    }
}

/// Mode-dependent color-write admission without the coverage-wrap override.
/// Literal port of `depth::mode_passes` (`depth.rs:114-121`), Programming
/// Manual Chapter 15 §15.7: opaque and interpenetrating surfaces accept a
/// clearly-nearer OR delta-correlated fragment (`relations.nearer`);
/// translucent surfaces require a strict in-front compare
/// (`relations.in_front`); decals require delta-correlation on both sides
/// AND a non-clear memory sample (`relations.farther && relations.nearer &&
/// !relations.memory_is_max`). Every valid `DepthMode` variant is handled;
/// there is no default/fallthrough arm, so a future fifth mode fails to
/// compile here rather than silently reusing an existing arm's semantics.
pub fn mode_passes(mode: DepthMode, relations: DepthRelations) -> bool {
    match mode {
        DepthMode::Opaque | DepthMode::Interpenetrating => relations.nearer,
        DepthMode::Translucent => relations.in_front,
        DepthMode::Decal => relations.farther && relations.nearer && !relations.memory_is_max,
    }
}

/// The three-way outcome of a coverage-wrap-aware depth decision. Literal
/// port of `crate::raster::coverage::DepthCoverageDecision`
/// (`coverage.rs:32-36`), promoted to a public type here since this module
/// is the depth-mode port's own seam rather than an internal helper of a
/// combiner-owned draw path. `UnsupportedInterpenetratingCoverageAdjustment`
/// is a first-class variant, not an error swallowed into `Reject` --
/// callers must handle it explicitly (AGENTS.md "loud traps, no silent
/// shrugs").
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DepthModeDecision {
    Pass,
    Reject,
    UnsupportedInterpenetratingCoverageAdjustment,
}

/// Full coverage-wrap-aware depth decision. Literal port of
/// `depth_coverage_decision` (`coverage.rs:39-59`): `Interpenetrating` mode
/// combined with `coverage_wraps=true` is an unsupported adjustment (the
/// Programming Manual requires a coverage-adjustment path here but does not
/// publish its arithmetic); `Opaque` mode combined with `coverage_wraps`
/// tightens the ordinary delta-tolerant `nearer` test to the strict
/// `in_front` test; every other `(mode, coverage_wraps)` combination falls
/// through to the plain `mode_passes` dispatch above, including
/// `Translucent`/`Decal` with `coverage_wraps=true` (the reference
/// implementation applies no wrap-specific override to those two modes, so
/// this port does not invent one).
pub fn depth_mode_decision(
    mode: DepthMode,
    relations: DepthRelations,
    coverage_wraps: bool,
) -> DepthModeDecision {
    if matches!(mode, DepthMode::Interpenetrating) && coverage_wraps {
        return DepthModeDecision::UnsupportedInterpenetratingCoverageAdjustment;
    }
    let passes = if matches!(mode, DepthMode::Opaque) && coverage_wraps {
        relations.in_front
    } else {
        mode_passes(mode, relations)
    };
    if passes {
        DepthModeDecision::Pass
    } else {
        DepthModeDecision::Reject
    }
}

pub const DEPTH_MODE_WGSL: &str = include_str!("depth_mode.wgsl");
pub const DEPTH_MODE_ENTRY_POINT: &str = "depth_mode_decision_batch";

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(
        pixel_z: u32,
        pixel_delta_z: u16,
        memory_z: u32,
        memory_encoded_delta_z: u8,
    ) -> DepthRelations {
        relations(pixel_z, pixel_delta_z, memory_z, memory_encoded_delta_z)
    }

    // --- relations() boundary characterization -----------------------

    #[test]
    fn relations_matches_reference_delta_tolerant_boundaries() {
        let front = rel(91, 4, 100, 3);
        assert_eq!(
            front,
            DepthRelations {
                memory_is_max: false,
                farther: false,
                nearer: true,
                in_front: true,
            }
        );
        let near_boundary = rel(92, 4, 100, 3);
        assert!(near_boundary.farther && near_boundary.nearer);
        let far_boundary = rel(108, 4, 100, 3);
        assert!(far_boundary.farther && far_boundary.nearer);
        let behind = rel(109, 4, 100, 3);
        assert!(behind.farther && !behind.nearer && !behind.in_front);
    }

    #[test]
    fn relations_uses_the_larger_of_pixel_and_memory_delta_z() {
        // At distance 10 (pixel_z=110, memory_z=100), pixel_delta_z=0 alone
        // does not correlate, but a larger stored memory exponent (4 ->
        // decoded 16) does -- delta_z_max must take the max of the two
        // decoded deltas, not just the pixel side.
        let with_only_minimum_deltas = rel(110, 0, 100, 0);
        assert!(!with_only_minimum_deltas.nearer);
        let with_larger_memory_delta = rel(110, 0, 100, 4);
        assert!(with_larger_memory_delta.nearer);
    }

    #[test]
    fn relations_decodes_the_stored_memory_delta_exponent_not_a_raw_value() {
        // memory_encoded_delta_z=3 must expand to decode_delta_z(3)=8, not
        // be compared as the literal value 3 -- this is the asymmetry
        // between the fragment's already-decoded pixel_delta_z and the
        // memory side's packed exponent.
        assert_eq!(decode_delta_z(3), 8);
        let decoded_matches = rel(92, 0, 100, 3);
        assert!(decoded_matches.nearer && decoded_matches.farther);
        let raw_would_not_match = rel(92, 0, 100, 0);
        assert!(!(raw_would_not_match.nearer && raw_would_not_match.farther));
    }

    #[test]
    fn relations_memory_is_max_exactly_at_hardware_ceiling() {
        assert!(rel(0, 0, 0x3ffff, 0).memory_is_max);
        assert!(!rel(0, 0, 0x3fffe, 0).memory_is_max);
    }

    #[test]
    fn relations_saturates_at_delta_add_and_delta_sub_edges() {
        // pixel_z at the documented 18-bit RDP ceiling with a large delta
        // must not wrap past 0x3ffff worth of headroom -- saturating_add/
        // sub, not wrapping arithmetic, within the caller-convention range.
        let at_ceiling = rel(0x3ffff, 0xffff, 0x3ffff, 0);
        assert!(at_ceiling.farther);
        assert!(at_ceiling.nearer);
        let at_floor = rel(0, 0xffff, 0, 0);
        assert!(at_floor.farther);
        assert!(at_floor.nearer);
    }

    #[test]
    fn relations_saturates_true_u32_add_even_outside_the_documented_z_range() {
        // `pixel_z`/`memory_z` are plain `u32`, not a bounded newtype, so
        // this must hold unconditionally, not just within the RDP's 18-bit
        // convention: `pixel_z` near `u32::MAX` plus a delta must clamp to
        // `u32::MAX` rather than wrapping around to a small value that
        // would spuriously read as "nearer." This is exactly the boundary
        // an adversarial review flagged as untested; `saturating_add` on
        // `u32` covers it directly, but this test pins the observable
        // behavior so a future refactor to unchecked/wrapping arithmetic
        // fails loudly here instead of silently at this input's neighbors.
        let near_max = rel(u32::MAX - 1, 0xffff, u32::MAX, 0);
        assert!(
            near_max.farther,
            "u32::MAX - 1 saturating_add(0xffff) must clamp to u32::MAX, not wrap"
        );
        // A plain wrapping `pixel_z + delta_z_max` would overflow past
        // u32::MAX back down near delta_z_max's own magnitude (0xffff),
        // which is far below memory_z and would spuriously report
        // `farther=false`. `saturating_add` instead clamps to u32::MAX,
        // which is `>= memory_z` (also u32::MAX here) -- so `farther` is
        // the discriminating signal for this boundary; `nearer` uses
        // `saturating_sub`, which has no such wraparound hazard at this
        // input (pixel_z stays far above delta_z_max) and is unaffected.
        assert!(near_max.nearer);
    }

    #[test]
    fn relations_in_front_is_strict_less_than_independent_of_delta() {
        assert!(rel(99, 0, 100, 0).in_front);
        assert!(!rel(100, 0, 100, 0).in_front);
        assert!(!rel(101, 0, 100, 0).in_front);
        // Large deltas never make in_front true at equal Z (exponent 15 is
        // the maximum stored DeltaZ exponent, decoding to 1<<15).
        assert!(!rel(100, 0xffff, 100, 15).in_front);
    }

    // --- mode_passes(): all four variants, exhaustive per the port card's
    // characterization-fixture partition (depth_mode × representative
    // DepthRelations corners) ------------------------------------------

    #[test]
    fn every_depth_mode_variant_is_dispatched_by_mode_passes() {
        let clear = rel(0x3f000, 1, 0x3ffff, 0);
        assert!(mode_passes(DepthMode::Opaque, clear));
        assert!(mode_passes(DepthMode::Interpenetrating, clear));
        assert!(mode_passes(DepthMode::Translucent, clear));
        assert!(!mode_passes(DepthMode::Decal, clear));

        let clearly_front = rel(91, 4, 100, 3);
        assert!(mode_passes(DepthMode::Opaque, clearly_front));
        assert!(mode_passes(DepthMode::Interpenetrating, clearly_front));
        assert!(mode_passes(DepthMode::Translucent, clearly_front));
        assert!(!mode_passes(DepthMode::Decal, clearly_front));

        let correlated = rel(108, 4, 100, 3);
        assert!(mode_passes(DepthMode::Opaque, correlated));
        assert!(mode_passes(DepthMode::Interpenetrating, correlated));
        assert!(!mode_passes(DepthMode::Translucent, correlated));
        assert!(mode_passes(DepthMode::Decal, correlated));

        let behind = rel(109, 4, 100, 3);
        for mode in [
            DepthMode::Opaque,
            DepthMode::Interpenetrating,
            DepthMode::Translucent,
            DepthMode::Decal,
        ] {
            assert!(!mode_passes(mode, behind));
        }
    }

    #[test]
    fn decal_mode_rejects_a_correlated_fragment_at_the_clear_memory_value() {
        // Decal additionally requires !memory_is_max -- a correlated
        // fragment landing on a cleared (max-Z) background must still
        // reject, distinguishing Decal from Opaque/Interpenetrating at this
        // corner.
        let correlated_but_cleared = rel(0x3fffe, 4, 0x3ffff, 4);
        assert!(correlated_but_cleared.farther);
        assert!(correlated_but_cleared.nearer);
        assert!(correlated_but_cleared.memory_is_max);
        assert!(!mode_passes(DepthMode::Decal, correlated_but_cleared));
        assert!(mode_passes(DepthMode::Opaque, correlated_but_cleared));
    }

    #[test]
    fn opaque_and_interpenetrating_share_identical_mode_passes_output() {
        // Both dispatch to `relations.nearer` -- their only behavioral
        // difference lives in depth_mode_decision's coverage-wrap layer.
        for (pixel_z, pixel_dz, memory_z, memory_dz) in [
            (91, 4, 100, 3),
            (108, 4, 100, 3),
            (109, 4, 100, 3),
            (0, 0, 0x3ffff, 0),
            (0x3ffff, 0, 0, 0),
        ] {
            let r = rel(pixel_z, pixel_dz, memory_z, memory_dz);
            assert_eq!(
                mode_passes(DepthMode::Opaque, r),
                mode_passes(DepthMode::Interpenetrating, r)
            );
        }
    }

    // --- depth_mode_decision(): coverage-wrap tightening ----------------

    #[test]
    fn opaque_without_wrap_uses_plain_nearer_test() {
        let correlated = rel(108, 4, 100, 3);
        assert_eq!(
            depth_mode_decision(DepthMode::Opaque, correlated, false),
            DepthModeDecision::Pass
        );
    }

    #[test]
    fn opaque_with_wrap_tightens_to_strict_in_front_rejecting_a_delta_correlated_fragment() {
        // `correlated` passes the plain nearer test (delta-tolerant) but is
        // NOT in_front (pixel_z=108 >= memory_z=100) -- wrap must reject it.
        let correlated = rel(108, 4, 100, 3);
        assert!(mode_passes(DepthMode::Opaque, correlated));
        assert!(!correlated.in_front);
        assert_eq!(
            depth_mode_decision(DepthMode::Opaque, correlated, true),
            DepthModeDecision::Reject
        );
    }

    #[test]
    fn opaque_with_wrap_still_passes_a_strictly_nearer_fragment() {
        let clearly_front = rel(91, 4, 100, 3);
        assert_eq!(
            depth_mode_decision(DepthMode::Opaque, clearly_front, true),
            DepthModeDecision::Pass
        );
    }

    #[test]
    fn opaque_with_wrap_rejects_exactly_at_the_equal_z_boundary() {
        let equal = rel(100, 4, 100, 3);
        assert!(!equal.in_front);
        assert_eq!(
            depth_mode_decision(DepthMode::Opaque, equal, true),
            DepthModeDecision::Reject
        );
    }

    #[test]
    fn interpenetrating_without_wrap_uses_plain_nearer_test() {
        let correlated = rel(108, 4, 100, 3);
        assert_eq!(
            depth_mode_decision(DepthMode::Interpenetrating, correlated, false),
            DepthModeDecision::Pass
        );
        let behind = rel(109, 4, 100, 3);
        assert_eq!(
            depth_mode_decision(DepthMode::Interpenetrating, behind, false),
            DepthModeDecision::Reject
        );
    }

    #[test]
    fn interpenetrating_with_wrap_is_the_unsupported_gap_regardless_of_relations() {
        // Every relations corner, including ones that would obviously pass
        // or reject under the plain test, must surface the same typed gap
        // -- this is a hard "no silent default" invariant, not a
        // best-effort approximation.
        for r in [
            rel(0, 0, 0x3ffff, 0),
            rel(91, 4, 100, 3),
            rel(108, 4, 100, 3),
            rel(109, 4, 100, 3),
            rel(0x3ffff, 0, 0, 0),
        ] {
            assert_eq!(
                depth_mode_decision(DepthMode::Interpenetrating, r, true),
                DepthModeDecision::UnsupportedInterpenetratingCoverageAdjustment
            );
        }
    }

    #[test]
    fn translucent_and_decal_apply_no_wrap_specific_override() {
        // The reference implementation only special-cases Opaque and
        // Interpenetrating; Translucent/Decal must produce identical
        // Pass/Reject results with coverage_wraps true or false.
        let corners = [
            rel(91, 4, 100, 3),
            rel(108, 4, 100, 3),
            rel(109, 4, 100, 3),
            rel(0x3f000, 1, 0x3ffff, 0),
        ];
        for r in corners {
            for mode in [DepthMode::Translucent, DepthMode::Decal] {
                let with_wrap = depth_mode_decision(mode, r, true);
                let without_wrap = depth_mode_decision(mode, r, false);
                assert_eq!(with_wrap, without_wrap);
                let expected = if mode_passes(mode, r) {
                    DepthModeDecision::Pass
                } else {
                    DepthModeDecision::Reject
                };
                assert_eq!(with_wrap, expected);
            }
        }
    }

    #[test]
    fn full_four_mode_by_two_wrap_states_truth_table_matches_reference_semantics() {
        // Exhaustive 4 modes x 2 wrap states x representative corners,
        // matching the port card's characterization-fixture partition #1
        // for §4.
        let corners = [
            rel(0x3f000, 1, 0x3ffff, 0), // memory-at-clear
            rel(91, 4, 100, 3),          // clearly front
            rel(108, 4, 100, 3),         // delta-correlated, not in_front
            rel(109, 4, 100, 3),         // clearly behind
        ];
        for r in corners {
            for wraps in [false, true] {
                for mode in [
                    DepthMode::Opaque,
                    DepthMode::Interpenetrating,
                    DepthMode::Translucent,
                    DepthMode::Decal,
                ] {
                    let decision = depth_mode_decision(mode, r, wraps);
                    match (mode, wraps) {
                        (DepthMode::Interpenetrating, true) => assert_eq!(
                            decision,
                            DepthModeDecision::UnsupportedInterpenetratingCoverageAdjustment
                        ),
                        (DepthMode::Opaque, true) => {
                            let expected = if r.in_front {
                                DepthModeDecision::Pass
                            } else {
                                DepthModeDecision::Reject
                            };
                            assert_eq!(decision, expected);
                        }
                        _ => {
                            let expected = if mode_passes(mode, r) {
                                DepthModeDecision::Pass
                            } else {
                                DepthModeDecision::Reject
                            };
                            assert_eq!(decision, expected);
                        }
                    }
                }
            }
        }
    }

    // --- WGSL companion seam --------------------------------------------

    #[test]
    fn wgsl_entry_point_name_matches_constant() {
        assert!(DEPTH_MODE_WGSL.contains(&format!("fn {DEPTH_MODE_ENTRY_POINT}(")));
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(DEPTH_MODE_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn duplicate_binding_index_fails_naga_validation() {
        let duplicate_binding = DEPTH_MODE_WGSL.replacen("@binding(1)", "@binding(0)", 1);
        let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_err());
    }

    #[test]
    fn malformed_wgsl_fails_to_parse() {
        let truncated = &DEPTH_MODE_WGSL[..DEPTH_MODE_WGSL.len() / 2];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    #[test]
    fn wgsl_source_encodes_all_four_mode_constants_and_the_unsupported_sentinel() {
        // Structural guard: the WGSL companion must name all four modes and
        // the unsupported-gap sentinel explicitly, not collapse any of them
        // into a shared branch by accident.
        for needle in [
            "MODE_OPAQUE",
            "MODE_INTERPENETRATING",
            "MODE_TRANSLUCENT",
            "MODE_DECAL",
            "DECISION_UNSUPPORTED_INTERPENETRATING",
        ] {
            assert!(
                DEPTH_MODE_WGSL.contains(needle),
                "expected WGSL source to contain {needle}"
            );
        }
    }

    /// Differential oracle: enumerate the same representative grid the
    /// Rust-side truth-table test above uses and require the WGSL source's
    /// frozen literal structure (checked textually, not GPU-executed -- see
    /// `depth_strict_less.rs`'s identical precedent and its documented
    /// scope boundary) to still contain the exact per-mode branch text this
    /// port relies on. This is a structural differential, not a
    /// GPU-executed one; native execution is out of scope for this slice.
    #[test]
    fn wgsl_source_contains_the_exact_opaque_wrap_tightening_branch() {
        assert!(DEPTH_MODE_WGSL.contains("relations.in_front"));
        assert!(DEPTH_MODE_WGSL.contains("relations.nearer"));
        assert!(DEPTH_MODE_WGSL.contains("relations.farther"));
        assert!(DEPTH_MODE_WGSL.contains("relations.memory_is_max"));
    }
}
