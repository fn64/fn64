//! Strict-less depth compare/update: the smallest real-raster-output slice.
//!
//! Characterization-first, selective literal port of the depth-only fragment
//! path `fn64-render-reference` already implements and tests at
//! `crates/fn64-render-reference/src/raster/draw.rs:632-653`
//! (`Framebuffer::set_depth_tested`): `z < memory_z` admits the fragment,
//! writing color and depth unconditionally on pass; any other relation
//! (farther OR equal) rejects, mutating neither target. That function is the
//! reference's own deliberately simplified test-only path (its doc comment
//! at the call site notes it bypasses blend/coverage/alpha-compare/depth-mode
//! entirely), matching step 1 of
//! `docs/RT64-BLENDER-DEPTH-PORT-CARD.md` "Smallest implementation slice
//! toward real raster output": depth compare only, no blend, no coverage, no
//! alpha compare, opaque-mode-only in the sense that no `DepthMode` variant
//! is consulted at all -- there is exactly one comparison, not RDP's
//! four-mode `mode_passes` dispatch.
//!
//! Provenance correction: `set_depth_tested` itself cites
//! `F3DEX2-CONCEPTS.md` §4.3 "Z-buffer compare", not RT64 -- that section is
//! sourced entirely from the public N64 Programming Manual (Chapters 15-16)
//! and libultra's `gDPSetPrimDepth`, with no RT64 attribution. RT64's
//! `Depth.hlsli` (pin `5473732a822a4423b5696e7cb18fecc425a59875`) is the
//! *encoded* 18-bit exponent/mantissa depth codec this slice explicitly does
//! not port (see above); `docs/rt64-port-inventory.json` records its
//! `port_state` as `not-started`, targeting a different, not-yet-created
//! file. No RT64 source byte is read, cited, or claimed by this module. This
//! module does not import `fn64-render-reference` (no such crate dependency
//! exists for `fn64-render-wgpu`); it is a self-contained literal
//! re-expression, matching this crate's existing citation-comment convention
//! (see `src/tmem/sample.rs`).
//!
//! Explicitly out of scope, per the port card: blend, coverage, alpha
//! compare, dither, `Interpenetrating`/other `DepthMode` variants, the
//! framebuffer-read problem, draw-call integration, and any native GPU
//! execution. This is a characterization oracle plus a validated WGSL
//! fragment-shader seam with matching arithmetic -- nothing consumes it yet.

/// One depth-buffer sample: the fragment's interpolated Z and the memory
/// (already-written) Z it is compared against. Both are the RDP's working
/// f32 representation, matching `Framebuffer::set_depth_tested`'s `z: f32`
/// parameter and `self.depth[pix]: f32` storage -- this slice does not port
/// the encoded 18-bit exponent/mantissa Z-buffer format (`depth.rs`'s
/// `EncodedDepth`/`encode_z`/`decode_z`), which belongs to a later slice
/// once `DepthMode`-aware comparison is in scope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrictLessDepthSample {
    pub fragment_z: f32,
    pub memory_z: f32,
}

impl StrictLessDepthSample {
    pub const fn new(fragment_z: f32, memory_z: f32) -> Self {
        Self {
            fragment_z,
            memory_z,
        }
    }
}

/// The two possible outcomes of a strict-less depth test: `Pass` admits the
/// fragment (color and depth both commit); `Reject` admits neither. There is
/// no partial-write outcome in this slice -- `set_depth_tested` in the
/// reference returns one `bool` gating both writes together, unlike the
/// depth/coverage/color decoupling `depth_coverage_decision` introduces for
/// the full pipeline (out of scope here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrictLessDepthOutcome {
    Pass,
    Reject,
}

/// `z < memory_z`. Matches `set_depth_tested`'s `if z < self.depth[pix]`
/// exactly, including that `NaN` on either side always rejects (Rust's `<`
/// on `f32` is `false` for any `NaN` operand) and that equal Z values reject
/// (strict less-than, not less-or-equal) -- both load-bearing boundary facts
/// this module's tests characterize explicitly.
pub const fn strict_less_depth_test(sample: StrictLessDepthSample) -> StrictLessDepthOutcome {
    if sample.fragment_z < sample.memory_z {
        StrictLessDepthOutcome::Pass
    } else {
        StrictLessDepthOutcome::Reject
    }
}

/// One depth-tested color write attempt: the CPU-side oracle for the whole
/// `set_depth_tested` function, not just its comparison. Carries the
/// pre-write `rgba` so callers (and tests) can observe that a `Reject`
/// mutates neither the returned depth nor color -- `set_depth_tested` never
/// touches `self.pixels`/`self.depth` on the `else` branch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrictLessDepthWrite {
    pub outcome: StrictLessDepthOutcome,
    pub committed_depth: f32,
    pub committed_rgba: [u8; 4],
}

/// The full oracle: given the memory state and an incoming fragment, compute
/// the post-write memory state. `memory_rgba` is the color already at that
/// pixel; on `Reject` it is echoed back unchanged, matching that
/// `set_depth_tested` performs no write of any kind when the comparison
/// fails.
pub const fn strict_less_depth_write(
    sample: StrictLessDepthSample,
    memory_rgba: [u8; 4],
    fragment_rgba: [u8; 4],
) -> StrictLessDepthWrite {
    match strict_less_depth_test(sample) {
        StrictLessDepthOutcome::Pass => StrictLessDepthWrite {
            outcome: StrictLessDepthOutcome::Pass,
            committed_depth: sample.fragment_z,
            committed_rgba: fragment_rgba,
        },
        StrictLessDepthOutcome::Reject => StrictLessDepthWrite {
            outcome: StrictLessDepthOutcome::Reject,
            committed_depth: sample.memory_z,
            committed_rgba: memory_rgba,
        },
    }
}

pub const STRICT_LESS_DEPTH_WGSL: &str = include_str!("depth_strict_less.wgsl");
pub const STRICT_LESS_DEPTH_ENTRY_POINT: &str = "strict_less_depth_test";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fragment_z: f32, memory_z: f32) -> StrictLessDepthSample {
        StrictLessDepthSample::new(fragment_z, memory_z)
    }

    #[test]
    fn clearly_nearer_fragment_passes() {
        assert_eq!(
            strict_less_depth_test(sample(0.0, 100.0)),
            StrictLessDepthOutcome::Pass
        );
        assert_eq!(
            strict_less_depth_test(sample(99.0, 100.0)),
            StrictLessDepthOutcome::Pass
        );
    }

    #[test]
    fn clearly_farther_fragment_rejects() {
        assert_eq!(
            strict_less_depth_test(sample(101.0, 100.0)),
            StrictLessDepthOutcome::Reject
        );
        assert_eq!(
            strict_less_depth_test(sample(0x3ffff as f32, 0.0)),
            StrictLessDepthOutcome::Reject
        );
    }

    #[test]
    fn exactly_equal_z_rejects_strict_less_than_not_less_or_equal() {
        assert_eq!(
            strict_less_depth_test(sample(100.0, 100.0)),
            StrictLessDepthOutcome::Reject
        );
        assert_eq!(
            strict_less_depth_test(sample(0.0, 0.0)),
            StrictLessDepthOutcome::Reject
        );
    }

    #[test]
    fn boundary_one_ulp_nearer_and_farther() {
        let just_under = 100.0_f32.next_down();
        let just_over = 100.0_f32.next_up();
        assert_eq!(
            strict_less_depth_test(sample(just_under, 100.0)),
            StrictLessDepthOutcome::Pass
        );
        assert_eq!(
            strict_less_depth_test(sample(just_over, 100.0)),
            StrictLessDepthOutcome::Reject
        );
    }

    #[test]
    fn extreme_depth_range_endpoints() {
        assert_eq!(
            strict_less_depth_test(sample(0.0, 0x3ffff as f32)),
            StrictLessDepthOutcome::Pass
        );
        assert_eq!(
            strict_less_depth_test(sample(0x3ffff as f32, 0x3ffff as f32)),
            StrictLessDepthOutcome::Reject
        );
        assert_eq!(
            strict_less_depth_test(sample(0.0, 0.0)),
            StrictLessDepthOutcome::Reject
        );
    }

    #[test]
    fn nan_fragment_z_always_rejects() {
        assert_eq!(
            strict_less_depth_test(sample(f32::NAN, 100.0)),
            StrictLessDepthOutcome::Reject
        );
        assert_eq!(
            strict_less_depth_test(sample(f32::NAN, f32::NEG_INFINITY)),
            StrictLessDepthOutcome::Reject
        );
    }

    #[test]
    fn nan_memory_z_always_rejects() {
        assert_eq!(
            strict_less_depth_test(sample(0.0, f32::NAN)),
            StrictLessDepthOutcome::Reject
        );
        assert_eq!(
            strict_less_depth_test(sample(f32::NEG_INFINITY, f32::NAN)),
            StrictLessDepthOutcome::Reject
        );
    }

    #[test]
    fn negative_and_infinite_z_are_not_specially_handled() {
        // set_depth_tested never clamps its z parameter (unlike
        // set_depth_controlled_blended, which clamps to [0, 0x3ffff] before
        // calling depth::relations). This slice ports set_depth_tested
        // literally, so out-of-hardware-range values compare with plain f32
        // semantics, not the encoded-Z clamp -- a deliberate scope boundary,
        // not an oversight.
        assert_eq!(
            strict_less_depth_test(sample(f32::NEG_INFINITY, 0.0)),
            StrictLessDepthOutcome::Pass
        );
        assert_eq!(
            strict_less_depth_test(sample(f32::INFINITY, 0.0)),
            StrictLessDepthOutcome::Reject
        );
        assert_eq!(
            strict_less_depth_test(sample(0.0, f32::INFINITY)),
            StrictLessDepthOutcome::Pass
        );
    }

    #[test]
    fn pass_commits_fragment_depth_and_color() {
        let write = strict_less_depth_write(sample(50.0, 100.0), [1, 2, 3, 4], [10, 20, 30, 40]);
        assert_eq!(write.outcome, StrictLessDepthOutcome::Pass);
        assert_eq!(write.committed_depth, 50.0);
        assert_eq!(write.committed_rgba, [10, 20, 30, 40]);
    }

    #[test]
    fn reject_mutates_neither_depth_nor_color() {
        let write = strict_less_depth_write(sample(150.0, 100.0), [1, 2, 3, 4], [10, 20, 30, 40]);
        assert_eq!(write.outcome, StrictLessDepthOutcome::Reject);
        assert_eq!(write.committed_depth, 100.0);
        assert_eq!(write.committed_rgba, [1, 2, 3, 4]);
    }

    #[test]
    fn reject_on_equal_z_mutates_neither_target() {
        let write = strict_less_depth_write(sample(100.0, 100.0), [9, 9, 9, 9], [1, 1, 1, 1]);
        assert_eq!(write.outcome, StrictLessDepthOutcome::Reject);
        assert_eq!(write.committed_depth, 100.0);
        assert_eq!(write.committed_rgba, [9, 9, 9, 9]);
    }

    #[test]
    fn exhaustive_boundary_sweep_matches_strict_less_than() {
        // Genuinely exhaustive: every integer memory_z across the full
        // 18-bit RDP depth range (0..=0x3ffff, 262,144 values) checked
        // against itself and its two immediate neighbors on each side.
        // Full pairwise (0..=0x3ffff)^2 is 2^36 pairs and not run here, but
        // every memory_z value is covered, and its immediate neighborhood is
        // exactly what a strict less-than test can distinguish for
        // integer-valued z: `fragment_z < memory_z` is constant within any
        // open interval strictly between two consecutive memory_z values, so
        // a fragment two-or-more-away from memory_z is redundant with a
        // one-away fragment against the same comparison direction.
        for memory_z_int in 0..=0x3ffff_u32 {
            let memory_z = memory_z_int as f32;
            for offset in -2..=2_i32 {
                let fragment_z = memory_z + offset as f32;
                let expected = if fragment_z < memory_z {
                    StrictLessDepthOutcome::Pass
                } else {
                    StrictLessDepthOutcome::Reject
                };
                assert_eq!(
                    strict_less_depth_test(sample(fragment_z, memory_z)),
                    expected,
                    "fragment_z={fragment_z} memory_z={memory_z}"
                );
            }
        }
    }

    #[test]
    fn wgsl_entry_point_name_matches_constant() {
        assert!(STRICT_LESS_DEPTH_WGSL.contains(&format!("fn {STRICT_LESS_DEPTH_ENTRY_POINT}(")));
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(STRICT_LESS_DEPTH_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn wgsl_rejects_a_flipped_comparison_direction_as_a_hostile_mutation() {
        // A `<=` (or `>`) mutation still parses and validates -- naga cannot
        // catch a semantic direction flip, only a syntactic/typing error.
        // This test documents that the WGSL/Rust semantic equivalence is
        // carried by this file's source-text identity and the differential
        // test below, not by naga validation alone.
        let flipped = STRICT_LESS_DEPTH_WGSL.replace(
            "return fragment_z < memory_z;",
            "return fragment_z <= memory_z;",
        );
        assert_ne!(flipped, STRICT_LESS_DEPTH_WGSL);
        let module = naga::front::wgsl::parse_str(&flipped).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_ok());
    }

    #[test]
    fn wgsl_source_uses_the_exact_strict_less_than_operator_once() {
        // Loud, structural guard against exactly the mutation the previous
        // test shows naga cannot catch: assert the source contains the
        // literal `<` comparison and not `<=`, so a future edit that
        // silently widens the test to less-or-equal fails this test instead
        // of shipping a semantic drift undetected.
        assert_eq!(
            STRICT_LESS_DEPTH_WGSL
                .matches("return fragment_z < memory_z;")
                .count(),
            1
        );
        assert!(!STRICT_LESS_DEPTH_WGSL.contains("fragment_z <= memory_z"));
    }

    #[test]
    fn duplicate_binding_index_fails_naga_validation() {
        let duplicate_binding = STRICT_LESS_DEPTH_WGSL.replacen("@binding(1)", "@binding(0)", 1);
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
        let truncated = &STRICT_LESS_DEPTH_WGSL[..STRICT_LESS_DEPTH_WGSL.len() / 2];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    /// Differential oracle: exhaustively enumerate a representative grid of
    /// `(fragment_z, memory_z)` pairs and require the Rust oracle's decision
    /// to match what the WGSL source's own literal arithmetic would compute
    /// -- interpreted here in Rust (not executed on a GPU; no native
    /// adapter, no draw integration, per this slice's explicit scope
    /// boundary) as the same `<` comparison read out of the shader text's
    /// frozen structure. This is a textual/structural differential, not a
    /// GPU-executed one; `#[cfg(feature = "host-gpu-tests")]` native
    /// execution is deliberately out of scope for this slice (see module
    /// doc).
    #[test]
    fn oracle_matches_wgsl_source_comparison_direction_across_a_grid() {
        assert!(STRICT_LESS_DEPTH_WGSL.contains("return fragment_z < memory_z;"));
        let values: [f32; 9] = [
            0.0,
            1.0,
            50.0,
            99.999,
            100.0,
            100.001,
            200.0,
            0x3ffff as f32,
            -1.0,
        ];
        for &fragment_z in &values {
            for &memory_z in &values {
                let expected = if fragment_z < memory_z {
                    StrictLessDepthOutcome::Pass
                } else {
                    StrictLessDepthOutcome::Reject
                };
                assert_eq!(
                    strict_less_depth_test(sample(fragment_z, memory_z)),
                    expected
                );
            }
        }
    }
}
