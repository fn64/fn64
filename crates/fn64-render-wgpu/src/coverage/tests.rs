use super::*;

// --- Coverage newtype: encoding round-trips and invariant boundary ---

#[test]
fn coverage_new_accepts_full_range_zero_through_eight() {
    for count in 0..=8u8 {
        assert_eq!(Coverage::new(count).count(), count);
    }
}

#[test]
#[should_panic(expected = "RDP coverage count exceeds eight samples")]
fn coverage_new_panics_above_eight() {
    let _ = Coverage::new(9);
}

#[test]
fn coverage_full_and_zero_constants() {
    assert_eq!(Coverage::FULL.count(), 8);
    assert_eq!(Coverage::ZERO.count(), 0);
}

#[test]
fn stored_round_trips_through_from_stored_for_every_nonzero_count() {
    for count in 1..=8u8 {
        let coverage = Coverage::new(count);
        let stored = coverage.stored();
        assert_eq!(stored, count - 1);
        assert_eq!(Coverage::from_stored(stored).count(), count);
    }
}

#[test]
fn from_stored_masks_to_three_bits() {
    // from_stored only consults the low three bits, matching the reference
    // exactly -- a caller passing a wider byte should not panic or wrap
    // unexpectedly.
    for stored in 0..=255u8 {
        let expected = (stored & 7) + 1;
        assert_eq!(Coverage::from_stored(stored).count(), expected);
    }
}

#[test]
#[should_panic(expected = "zero coverage is never stored in RDRAM")]
fn stored_debug_asserts_on_zero_coverage() {
    let _ = Coverage::ZERO.stored();
}

#[test]
fn coverage_alpha_matches_exact_rounding_formula_for_every_count() {
    for count in 0..=8u8 {
        let expected = ((count as u16) * 255 + 4) / 8;
        assert_eq!(Coverage::new(count).alpha() as u16, expected);
    }
}

#[test]
fn coverage_alpha_endpoints() {
    assert_eq!(Coverage::ZERO.alpha(), 0);
    assert_eq!(Coverage::FULL.alpha(), 255);
}

#[test]
fn times_alpha_matches_exact_rounding_formula_across_full_matrix() {
    for count in 0..=8u8 {
        for alpha in 0..=255u16 {
            let expected = (((count as u16) * alpha + 127) / 255) as u8;
            let result = Coverage::new(count).times_alpha(alpha as u8);
            assert_eq!(result.count(), expected, "count={count} alpha={alpha}");
        }
    }
}

#[test]
fn times_alpha_zero_alpha_collapses_to_zero_coverage() {
    for count in 0..=8u8 {
        assert_eq!(Coverage::new(count).times_alpha(0).count(), 0);
    }
}

#[test]
fn times_alpha_full_alpha_preserves_count() {
    for count in 0..=8u8 {
        assert_eq!(Coverage::new(count).times_alpha(255).count(), count);
    }
}

// CoverageDestination's wire decode (`0=Clamp,1=Wrap,2=Full,3=Save`) is
// owned by `state::OtherMode::coverage_destination`, exercised by
// `state.rs`'s own `coverage_destination_decodes_all_four_wire_encodings`
// test -- this module reuses `state::CoverageDestination` as a plain typed
// parameter (see module doc) and does not re-decode it, so no duplicate
// wire-decode test lives here.

// --- coverage_result: exhaustive fixture partitions per the port card ---

fn mode(
    image_read_enabled: bool,
    force_blend: bool,
    antialias_enabled: bool,
    coverage_destination: CoverageDestination,
) -> CoverageModeBits {
    CoverageModeBits {
        image_read_enabled,
        force_blend,
        antialias_enabled,
        coverage_destination,
    }
}

#[test]
fn blend_enabled_truth_table_is_fully_exhaustive() {
    // Port card §2 partition 2: force_blend x antialias_enabled x wraps
    // (derived), 8 combinations, fully exhaustive.
    for &force_blend in &[false, true] {
        for &antialias_enabled in &[false, true] {
            for &image_read_enabled in &[false, true] {
                // Drive `wraps` via sum > 8 using representative pixel/memory
                // counts, holding destination fixed at Clamp (does not gate
                // blend_enabled).
                for &(pixel_count, memory_count) in &[(0u8, 0u8), (4, 4), (8, 8)] {
                    let pixel = Coverage::new(pixel_count);
                    let memory = Coverage::new(memory_count);
                    let m = mode(
                        image_read_enabled,
                        force_blend,
                        antialias_enabled,
                        CoverageDestination::Clamp,
                    );
                    let result = coverage_result(pixel, memory, m);
                    let sum = if image_read_enabled {
                        pixel_count as u16 + memory_count as u16
                    } else {
                        pixel_count as u16
                    };
                    let expected_wraps = image_read_enabled && sum > 8;
                    let expected_blend_enabled =
                        force_blend || (antialias_enabled && !expected_wraps);
                    assert_eq!(result.wraps, expected_wraps);
                    assert_eq!(result.blend_enabled, expected_blend_enabled);
                }
            }
        }
    }
}

#[test]
fn cvg_dst_clamp_matches_min_sum_eight_when_gated() {
    for &image_read_enabled in &[false, true] {
        for &blend_gate in &[false, true] {
            // force_blend alone controls blend_enabled independent of wraps
            // when antialias_enabled=false and image_read disables wraps.
            let m = mode(
                image_read_enabled,
                blend_gate,
                false,
                CoverageDestination::Clamp,
            );
            for pixel_count in 0..=8u8 {
                for &memory_count in &[0u8, 1, 4, 8] {
                    let pixel = Coverage::new(pixel_count);
                    let memory = Coverage::new(memory_count);
                    let result = coverage_result(pixel, memory, m);
                    let sum = if image_read_enabled {
                        pixel_count as u16 + memory_count as u16
                    } else {
                        pixel_count as u16
                    };
                    // antialias_enabled is fixed to false above, so
                    // blend_enabled collapses to force_blend alone.
                    let blend_enabled = blend_gate;
                    let expected = if image_read_enabled && blend_enabled {
                        sum.min(8) as u8
                    } else {
                        pixel_count
                    };
                    assert_eq!(result.destination.count(), expected);
                }
            }
        }
    }
}

#[test]
fn cvg_dst_wrap_matches_sum_minus_eight_when_wrapping() {
    for &image_read_enabled in &[false, true] {
        let m = mode(image_read_enabled, false, false, CoverageDestination::Wrap);
        for pixel_count in 0..=8u8 {
            for &memory_count in &[0u8, 1, 4, 8] {
                let pixel = Coverage::new(pixel_count);
                let memory = Coverage::new(memory_count);
                let result = coverage_result(pixel, memory, m);
                let expected = if image_read_enabled {
                    let sum = pixel_count as u16 + memory_count as u16;
                    if sum > 8 {
                        (sum - 8) as u8
                    } else {
                        sum as u8
                    }
                } else {
                    pixel_count
                };
                assert_eq!(result.destination.count(), expected);
            }
        }
    }
}

#[test]
fn cvg_dst_wrap_boundary_sum_exactly_nine_is_first_wrap_value() {
    let m = mode(true, false, false, CoverageDestination::Wrap);
    let result = coverage_result(Coverage::new(8), Coverage::new(1), m);
    assert!(result.wraps);
    assert_eq!(result.destination.count(), 1);
}

#[test]
fn cvg_dst_wrap_boundary_sum_exactly_eight_does_not_wrap() {
    let m = mode(true, false, false, CoverageDestination::Wrap);
    let result = coverage_result(Coverage::new(4), Coverage::new(4), m);
    assert!(!result.wraps);
    assert_eq!(result.destination.count(), 8);
}

#[test]
fn cvg_dst_clamp_and_wrap_agree_at_sum_equals_eight_boundary() {
    let clamp = mode(true, true, false, CoverageDestination::Clamp);
    let wrap = mode(true, true, false, CoverageDestination::Wrap);
    let pixel = Coverage::new(5);
    let memory = Coverage::new(3);
    let clamp_result = coverage_result(pixel, memory, clamp);
    let wrap_result = coverage_result(pixel, memory, wrap);
    assert_eq!(clamp_result.destination.count(), 8);
    assert_eq!(wrap_result.destination.count(), 8);
}

#[test]
fn cvg_dst_full_always_returns_full_regardless_of_inputs() {
    for &image_read_enabled in &[false, true] {
        for &force_blend in &[false, true] {
            for &antialias_enabled in &[false, true] {
                let m = mode(
                    image_read_enabled,
                    force_blend,
                    antialias_enabled,
                    CoverageDestination::Full,
                );
                for pixel_count in 0..=8u8 {
                    for &memory_count in &[0u8, 1, 4, 8] {
                        let result = coverage_result(
                            Coverage::new(pixel_count),
                            Coverage::new(memory_count),
                            m,
                        );
                        assert_eq!(result.destination, Coverage::FULL);
                    }
                }
            }
        }
    }
}

#[test]
fn cvg_dst_save_always_passes_memory_through_unchanged() {
    let m = mode(true, false, false, CoverageDestination::Save);
    for pixel_count in 0..=8u8 {
        for memory_count in 0..=8u8 {
            let result =
                coverage_result(Coverage::new(pixel_count), Coverage::new(memory_count), m);
            assert_eq!(result.destination.count(), memory_count);
        }
    }
}

#[test]
fn coverage_result_exhaustive_matrix_all_modes_all_counts_all_gates() {
    // Port card §2 partition 1: cvg_dst x image_read_enabled x pixel.count()
    // 0..=8 x memory.count() representative {0,1,4,8}, cross force_blend and
    // antialias_enabled for full coverage of the blend_enabled interaction.
    let destinations = [
        CoverageDestination::Clamp,
        CoverageDestination::Wrap,
        CoverageDestination::Full,
        CoverageDestination::Save,
    ];
    for &destination in &destinations {
        for &image_read_enabled in &[false, true] {
            for &force_blend in &[false, true] {
                for &antialias_enabled in &[false, true] {
                    let m = mode(
                        image_read_enabled,
                        force_blend,
                        antialias_enabled,
                        destination,
                    );
                    for pixel_count in 0..=8u8 {
                        for &memory_count in &[0u8, 1, 4, 8] {
                            let pixel = Coverage::new(pixel_count);
                            let memory = Coverage::new(memory_count);
                            let result = coverage_result(pixel, memory, m);
                            // Independent re-derivation (not calling
                            // coverage_result) to serve as a genuine
                            // differential, not a tautology.
                            let sum = if image_read_enabled {
                                pixel_count as u16 + memory_count as u16
                            } else {
                                pixel_count as u16
                            };
                            let wraps = image_read_enabled && sum > 8;
                            let blend_enabled = force_blend || (antialias_enabled && !wraps);
                            let expected_destination = match destination {
                                CoverageDestination::Clamp => {
                                    if image_read_enabled && blend_enabled {
                                        sum.min(8) as u8
                                    } else {
                                        pixel_count
                                    }
                                }
                                CoverageDestination::Wrap => {
                                    if image_read_enabled {
                                        if wraps {
                                            (sum - 8) as u8
                                        } else {
                                            sum as u8
                                        }
                                    } else {
                                        pixel_count
                                    }
                                }
                                CoverageDestination::Full => 8,
                                CoverageDestination::Save => memory_count,
                            };
                            assert_eq!(result.wraps, wraps);
                            assert_eq!(result.blend_enabled, blend_enabled);
                            assert_eq!(result.destination.count(), expected_destination);
                            assert_eq!(result.pixel, pixel);
                            assert_eq!(result.memory, memory);
                        }
                    }
                }
            }
        }
    }
}

// --- apply_coverage_alpha ---

#[test]
fn apply_coverage_alpha_neither_bit_set_is_a_no_op() {
    for count in 0..=8u8 {
        for &a in &[0u8, 1, 127, 254, 255] {
            let rgba = [10, 20, 30, a];
            let (result_rgba, result_coverage) =
                apply_coverage_alpha(false, false, rgba, Coverage::new(count));
            assert_eq!(result_rgba, rgba);
            assert_eq!(result_coverage.count(), count);
        }
    }
}

#[test]
fn apply_coverage_alpha_times_alpha_only_adjusts_coverage_not_rgba() {
    for count in 0..=8u8 {
        for &a in &[0u8, 1, 127, 254, 255] {
            let rgba = [10, 20, 30, a];
            let (result_rgba, result_coverage) =
                apply_coverage_alpha(true, false, rgba, Coverage::new(count));
            assert_eq!(
                result_rgba, rgba,
                "rgba must be unchanged when alpha_coverage_select is false"
            );
            let expected = ((count as u16) * (a as u16) + 127) / 255;
            assert_eq!(result_coverage.count() as u16, expected);
        }
    }
}

#[test]
fn apply_coverage_alpha_select_only_overwrites_alpha_with_raw_coverage() {
    for count in 0..=8u8 {
        let rgba = [10, 20, 30, 77];
        let (result_rgba, result_coverage) =
            apply_coverage_alpha(false, true, rgba, Coverage::new(count));
        assert_eq!(
            result_coverage.count(),
            count,
            "coverage must be unchanged when coverage_times_alpha is false"
        );
        assert_eq!(result_rgba[3], Coverage::new(count).alpha());
        assert_eq!(result_rgba[..3], rgba[..3]);
    }
}

#[test]
fn apply_coverage_alpha_both_bits_compose_times_then_select() {
    // Port card §2 partition 3, full cross-product for the two independent
    // interactions plus rounding.
    for count in 0..=8u8 {
        for &a in &[0u8, 1, 127, 254, 255] {
            let rgba = [1, 2, 3, a];
            let (result_rgba, result_coverage) =
                apply_coverage_alpha(true, true, rgba, Coverage::new(count));
            let multiplied = ((count as u16) * (a as u16) + 127) / 255;
            assert_eq!(result_coverage.count() as u16, multiplied);
            let expected_alpha = ((multiplied * 255 + 4) / 8) as u8;
            assert_eq!(result_rgba[3], expected_alpha);
            assert_eq!(result_rgba[..3], rgba[..3]);
        }
    }
}

// --- CoverageMask: all 256 masks ---

#[test]
fn coverage_mask_popcount_matches_coverage_for_all_256_masks() {
    for bits in 0..=255u8 {
        let mask = CoverageMask::from_bits(bits);
        assert_eq!(mask.coverage().count(), bits.count_ones() as u8);
    }
}

#[test]
fn coverage_mask_contains_matches_bit_position_for_all_256_masks() {
    for bits in 0..=255u8 {
        let mask = CoverageMask::from_bits(bits);
        for i in 0..8 {
            assert_eq!(mask.contains(i), (bits & (1 << i)) != 0);
        }
    }
}

#[test]
#[should_panic]
fn coverage_mask_contains_panics_on_out_of_range_index() {
    let _ = CoverageMask::FULL.contains(8);
}

#[test]
fn coverage_mask_empty_and_full_constants() {
    assert_eq!(CoverageMask::EMPTY.0, 0);
    assert_eq!(CoverageMask::FULL.0, 0xff);
    assert_eq!(CoverageMask::EMPTY.coverage(), Coverage::ZERO);
    assert_eq!(CoverageMask::FULL.coverage(), Coverage::FULL);
}

#[test]
fn coverage_samples_are_the_eight_public_positions_in_order() {
    assert_eq!(
        COVERAGE_SAMPLES,
        [
            (1, 1),
            (5, 1),
            (3, 3),
            (7, 3),
            (1, 5),
            (5, 5),
            (3, 7),
            (7, 7)
        ]
    );
}

// --- attribute_sample: all 256 masks, policy determinism ---

#[test]
#[should_panic(expected = "zero coverage has no attribute sample")]
fn attribute_sample_panics_on_empty_mask() {
    let _ = attribute_sample(CoverageMask::EMPTY);
}

#[test]
fn attribute_sample_full_mask_is_pixel_center() {
    assert_eq!(
        attribute_sample(CoverageMask::FULL),
        AttributeSamplePoint::PixelCenter
    );
}

#[test]
fn attribute_sample_pixel_center_offsets_are_the_geometric_center() {
    assert_eq!(AttributeSamplePoint::PixelCenter.offsets_eighth(), (4, 4));
}

#[test]
fn attribute_sample_resolves_for_every_nonzero_partial_mask() {
    // Full 256-mask sweep per port card §2 partition 5: every nonzero,
    // non-full mask must resolve to a Covered sample whose bit is actually
    // set in the mask -- proves the preference-order fallback never panics
    // for a real partial mask and never returns a sample outside the mask.
    for bits in 1..=254u8 {
        let mask = CoverageMask::from_bits(bits);
        match attribute_sample(mask) {
            AttributeSamplePoint::Covered(sample) => {
                assert!(
                    mask.contains(sample.sample_index as usize),
                    "bits={bits:#010b} selected sample {} not covered",
                    sample.sample_index
                );
                let (x, y) = COVERAGE_SAMPLES[sample.sample_index as usize];
                assert_eq!((sample.x_eighth, sample.y_eighth), (x, y));
            }
            AttributeSamplePoint::PixelCenter => {
                panic!("bits={bits:#010b} partial mask must not resolve to PixelCenter");
            }
        }
    }
}

#[test]
fn attribute_sample_is_deterministic_across_repeated_calls() {
    for bits in 1..=254u8 {
        let mask = CoverageMask::from_bits(bits);
        assert_eq!(attribute_sample(mask), attribute_sample(mask));
    }
}

#[test]
fn attribute_sample_single_bit_masks_select_that_exact_sample() {
    for index in 0..8usize {
        let mask = CoverageMask::from_bits(1u8 << index);
        match attribute_sample(mask) {
            AttributeSamplePoint::Covered(sample) => {
                assert_eq!(sample.sample_index as usize, index);
            }
            AttributeSamplePoint::PixelCenter => panic!("single-bit mask must not be PixelCenter"),
        }
    }
}

// --- WGSL component: text identity, Naga validation, hostile mutations ---

#[test]
fn wgsl_entry_point_name_matches_constant() {
    assert!(COVERAGE_WGSL.contains(&format!("fn {COVERAGE_ENTRY_POINT}(")));
}

#[test]
fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(COVERAGE_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn malformed_wgsl_fails_to_parse() {
    let truncated = &COVERAGE_WGSL[..COVERAGE_WGSL.len() / 2];
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn duplicate_binding_index_fails_naga_validation() {
    let duplicate_binding = COVERAGE_WGSL.replacen("@binding(1)", "@binding(0)", 1);
    let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_err());
}

#[test]
fn wgsl_source_uses_exact_clamp_min_call_once() {
    // Loud structural guard against a mutation naga cannot catch: Clamp mode
    // must compute min(sum, 8), not e.g. sum unconditionally.
    assert_eq!(
        COVERAGE_WGSL
            .matches("destination = min(sum, COVERAGE_FULL);")
            .count(),
        1
    );
}

#[test]
fn wgsl_source_uses_exact_wrap_subtraction_once() {
    assert_eq!(
        COVERAGE_WGSL
            .matches("destination = sum - COVERAGE_FULL;")
            .count(),
        1
    );
}

#[test]
fn wgsl_source_uses_exact_coverage_alpha_rounding_formula_once() {
    assert_eq!(
        COVERAGE_WGSL
            .matches("return (count * 255u + 4u) / 8u;")
            .count(),
        1
    );
}

#[test]
fn wgsl_source_uses_exact_times_alpha_rounding_formula_once() {
    assert_eq!(
        COVERAGE_WGSL
            .matches("return (count * alpha + 127u) / 255u;")
            .count(),
        1
    );
}

#[test]
fn wgsl_source_computes_alpha_coverage_select_from_adjusted_coverage_not_raw_destination() {
    // Load-bearing sequencing fact (mirrors apply_coverage_alpha's Rust
    // ordering): alpha_coverage_select must read the *possibly
    // times-alpha-multiplied* coverage, not `destination` directly. This
    // guard ties evaluate()'s composition order to the literal text so a
    // reorder that reads `destination` instead of `adjusted_coverage` fails
    // here even though it would still parse and validate under naga.
    assert_eq!(
        COVERAGE_WGSL
            .matches("adjusted_alpha = coverage_alpha(adjusted_coverage);")
            .count(),
        1
    );
    assert!(!COVERAGE_WGSL.contains("adjusted_alpha = coverage_alpha(destination);"));
}

#[test]
fn wgsl_rejects_hostile_alpha_composition_reorder_still_parses_and_validates() {
    // Swapping to read `destination` instead of `adjusted_coverage` is a
    // real semantic bug (alpha_coverage_select would ignore
    // coverage_times_alpha's effect) but still parses/validates under naga
    // -- documents the same naga blind spot as the wrap-boundary hostile
    // test above, for the alpha-composition block specifically.
    let flipped = COVERAGE_WGSL.replace(
        "adjusted_alpha = coverage_alpha(adjusted_coverage);",
        "adjusted_alpha = coverage_alpha(destination);",
    );
    assert_ne!(flipped, COVERAGE_WGSL);
    let module = naga::front::wgsl::parse_str(&flipped).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_ok());
}

#[test]
fn wgsl_rejects_hostile_times_alpha_rounding_mutation_still_parses_and_validates() {
    let flipped = COVERAGE_WGSL.replace(
        "return (count * alpha + 127u) / 255u;",
        "return (count * alpha + 128u) / 255u;",
    );
    assert_ne!(flipped, COVERAGE_WGSL);
    let module = naga::front::wgsl::parse_str(&flipped).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_ok());
}

#[test]
fn wgsl_rejects_hostile_wrap_boundary_mutation_still_parses_and_validates() {
    // A `>=` mutation of the wrap boundary still parses and validates --
    // naga cannot catch a semantic boundary flip, only syntax/typing. This
    // documents that the WGSL/Rust equivalence is carried by this file's
    // source-text identity plus the differential test below, not naga alone.
    let flipped = COVERAGE_WGSL.replace(
        "let wraps = image_read && (sum > COVERAGE_FULL);",
        "let wraps = image_read && (sum >= COVERAGE_FULL);",
    );
    assert_ne!(flipped, COVERAGE_WGSL);
    let module = naga::front::wgsl::parse_str(&flipped).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_ok());
}

/// Differential oracle: for every `(cvg_dst, image_read, force_blend,
/// antialias, pixel, memory)` combination in the exhaustive matrix above,
/// interpret the WGSL source's frozen arithmetic in Rust (textual/structural
/// differential, not GPU-executed -- matching `depth_strict_less.rs`'s
/// existing convention; no native adapter, no draw integration) and require
/// it to match `coverage_result`'s output exactly.
#[test]
fn wgsl_arithmetic_matches_rust_oracle_across_full_matrix() {
    fn wgsl_equivalent(
        pixel_count: u8,
        memory_count: u8,
        image_read_enabled: bool,
        force_blend: bool,
        antialias_enabled: bool,
        destination_code: u32,
    ) -> (u8, bool, bool) {
        let sum: u32 = if image_read_enabled {
            pixel_count as u32 + memory_count as u32
        } else {
            pixel_count as u32
        };
        let wraps = image_read_enabled && sum > 8;
        let blend_enabled = force_blend || (antialias_enabled && !wraps);
        let destination = match destination_code {
            0 => {
                if image_read_enabled && blend_enabled {
                    sum.min(8)
                } else {
                    pixel_count as u32
                }
            }
            1 => {
                if image_read_enabled {
                    if wraps {
                        sum - 8
                    } else {
                        sum
                    }
                } else {
                    pixel_count as u32
                }
            }
            2 => 8,
            _ => memory_count as u32,
        };
        (destination as u8, wraps, blend_enabled)
    }

    let destinations = [
        (0u32, CoverageDestination::Clamp),
        (1, CoverageDestination::Wrap),
        (2, CoverageDestination::Full),
        (3, CoverageDestination::Save),
    ];
    for &(code, destination) in &destinations {
        for &image_read_enabled in &[false, true] {
            for &force_blend in &[false, true] {
                for &antialias_enabled in &[false, true] {
                    let m = mode(
                        image_read_enabled,
                        force_blend,
                        antialias_enabled,
                        destination,
                    );
                    for pixel_count in 0..=8u8 {
                        for &memory_count in &[0u8, 1, 4, 8] {
                            let rust_result = coverage_result(
                                Coverage::new(pixel_count),
                                Coverage::new(memory_count),
                                m,
                            );
                            let (wgsl_destination, wgsl_wraps, wgsl_blend_enabled) =
                                wgsl_equivalent(
                                    pixel_count,
                                    memory_count,
                                    image_read_enabled,
                                    force_blend,
                                    antialias_enabled,
                                    code,
                                );
                            assert_eq!(rust_result.destination.count(), wgsl_destination);
                            assert_eq!(rust_result.wraps, wgsl_wraps);
                            assert_eq!(rust_result.blend_enabled, wgsl_blend_enabled);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn wgsl_coverage_alpha_matches_rust_oracle_across_full_matrix() {
    fn wgsl_coverage_alpha(count: u32) -> u32 {
        (count * 255 + 4) / 8
    }
    for count in 0..=8u32 {
        assert_eq!(
            wgsl_coverage_alpha(count) as u8,
            Coverage::new(count as u8).alpha()
        );
    }
}

#[test]
fn wgsl_times_alpha_matches_rust_oracle_across_full_matrix() {
    fn wgsl_times_alpha(count: u32, alpha: u32) -> u32 {
        (count * alpha + 127) / 255
    }
    for count in 0..=8u32 {
        for alpha in 0..=255u32 {
            assert_eq!(
                wgsl_times_alpha(count, alpha) as u8,
                Coverage::new(count as u8).times_alpha(alpha as u8).count()
            );
        }
    }
}

/// Differential oracle for `evaluate()`'s alpha-composition block
/// (`coverage.wgsl:90-97`), the surface `wgsl_arithmetic_matches_rust_oracle_across_full_matrix`
/// does not touch (that test only covers `coverage_result`'s
/// destination/wraps/blend_enabled outputs). Interprets the WGSL's frozen
/// `adjusted_coverage`/`adjusted_alpha` composition in Rust -- both
/// independent-gate booleans crossed with representative alpha/count values
/// -- and requires it to match `apply_coverage_alpha` exactly, including the
/// load-bearing sequencing fact that `alpha_coverage_select` reads the
/// post-times-alpha coverage, not the raw destination.
#[test]
fn wgsl_alpha_composition_matches_apply_coverage_alpha_across_full_matrix() {
    fn wgsl_alpha_composition(
        destination_count: u32,
        coverage_times_alpha: bool,
        alpha_coverage_select: bool,
        fragment_alpha: u32,
    ) -> (u32, u32) {
        let mut adjusted_coverage = destination_count;
        if coverage_times_alpha {
            adjusted_coverage = (destination_count * fragment_alpha + 127) / 255;
        }
        let mut adjusted_alpha = fragment_alpha;
        if alpha_coverage_select {
            adjusted_alpha = (adjusted_coverage * 255 + 4) / 8;
        }
        (adjusted_alpha, adjusted_coverage)
    }

    for count in 0..=8u8 {
        for &alpha in &[0u8, 1, 127, 254, 255] {
            for &coverage_times_alpha in &[false, true] {
                for &alpha_coverage_select in &[false, true] {
                    let rgba = [1, 2, 3, alpha];
                    let (rust_rgba, rust_coverage) = apply_coverage_alpha(
                        coverage_times_alpha,
                        alpha_coverage_select,
                        rgba,
                        Coverage::new(count),
                    );
                    let (wgsl_alpha, wgsl_coverage) = wgsl_alpha_composition(
                        count as u32,
                        coverage_times_alpha,
                        alpha_coverage_select,
                        alpha as u32,
                    );
                    assert_eq!(
                        rust_coverage.count() as u32,
                        wgsl_coverage,
                        "count={count} alpha={alpha} times_alpha={coverage_times_alpha} select={alpha_coverage_select}"
                    );
                    assert_eq!(
                        rust_rgba[3] as u32,
                        wgsl_alpha,
                        "count={count} alpha={alpha} times_alpha={coverage_times_alpha} select={alpha_coverage_select}"
                    );
                }
            }
        }
    }
}
