use fn64_render_ir::PhysicalMemoryLayout;

use super::*;

const RDRAM_BYTES: u32 = 8 * 1024 * 1024;
const FIXTURE_START: u32 = 0x400;

fn layout() -> PhysicalMemoryLayout {
    PhysicalMemoryLayout::try_new(RDRAM_BYTES).unwrap()
}

fn key_at(start: u32, width: u32, height: u32, format: ColorTargetFormat) -> ColorTargetKey {
    let layout = layout();
    ColorTargetKey::try_new(
        layout.address(start).unwrap(),
        ColorTargetExtent::try_new(width, height).unwrap(),
        format,
    )
    .unwrap()
}

fn full_rectangle(key: ColorTargetKey) -> TargetRectangle {
    TargetRectangle::try_new(0, 0, key.extent().width(), key.extent().height()).unwrap()
}

fn initialized(
    registry: &ColorTargetRegistry,
    key: ColorTargetKey,
) -> InitializedCandidateColorTarget {
    let candidate = registry.begin_candidate(key).unwrap();
    let plan = candidate.plan_rows(full_rectangle(key)).unwrap();
    let pixels = vec![Rgba8::new(255, 0, 0, 255); key.extent().pixels() as usize];
    let completion = completed(&candidate, plan, &pixels);
    candidate
        .admit_completed_initialization(completion)
        .unwrap()
}

fn completed(
    candidate: &CandidateColorTarget,
    plan: ExactRowPlan,
    pixels: &[Rgba8],
) -> CompletedColorTargetWrite {
    CompletedColorTargetWrite {
        key: plan.key,
        generation: plan.generation,
        range: plan.key.range(),
        rectangle: plan.rectangle,
        device_bytes: pack_device_pixels(candidate, pixels).unwrap(),
    }
}

#[test]
fn exact_4x2_rgba16_rows_cover_the_frozen_fixture_once() {
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    assert_eq!(key.range().start().get(), FIXTURE_START);
    assert_eq!(key.range().end(), 0x410);

    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    assert_eq!(candidate.generation(), TargetGeneration::FIRST);
    let plan = candidate.plan_rows(full_rectangle(key)).unwrap();
    let rows = plan.rows().collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].row(), 0);
    assert_eq!(rows[0].first_pixel(), 0);
    assert_eq!(rows[0].pixel_count(), 4);
    assert_eq!(
        (rows[0].bytes().start().get(), rows[0].bytes().end()),
        (0x400, 0x408)
    );
    assert_eq!(
        (rows[1].bytes().start().get(), rows[1].bytes().end()),
        (0x408, 0x410)
    );

    let completion = completed(&candidate, plan, &[Rgba8::new(255, 0, 0, 255); 8]);
    let initialized = candidate
        .admit_completed_initialization(completion)
        .unwrap();
    let proof = initialized.initialized_region();
    assert_eq!(proof.range(), key.range());
    assert_eq!(proof.rows(), 2);
    assert_eq!(proof.generation(), TargetGeneration::FIRST);
}

#[test]
fn exact_fixture_packs_to_m3_3a_device_domain() {
    let red = Rgba8::new(255, 0, 0, 255);
    let pixels = [red; 8];
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry
        .begin_candidate(key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16))
        .unwrap();
    let packed = pack_device_pixels(&candidate, &pixels).unwrap();
    assert_eq!(packed.device_bytes(), [0xf8, 0x01].repeat(8));
    let m3_3a = packed.into_m3_3a_rgba16().unwrap();
    assert_eq!(m3_3a.device_bytes(), [0xf8, 0x01].repeat(8));
}

#[test]
fn rgba16_oracle_has_exact_quantization_and_alpha_semantics() {
    let pixels = [
        Rgba8::new(0, 0, 0, 0),
        Rgba8::new(255, 255, 255, 255),
        Rgba8::new(171, 92, 37, 127),
        Rgba8::new(171, 92, 37, 128),
    ];
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry
        .begin_candidate(key_at(FIXTURE_START, 4, 1, ColorTargetFormat::Rgba16))
        .unwrap();
    let packed = pack_device_pixels(&candidate, &pixels).unwrap();
    let unpacked = unpack_device_pixels(ColorTargetFormat::Rgba16, packed.device_bytes()).unwrap();
    assert_eq!(unpacked[0], Rgba8::new(0, 0, 0, 0));
    assert_eq!(unpacked[1], Rgba8::new(255, 255, 255, 255));
    assert_eq!(unpacked[2], Rgba8::new(173, 90, 33, 0));
    assert_eq!(unpacked[3], Rgba8::new(173, 90, 33, 255));
}

#[test]
fn rgba32_oracle_is_byte_exact() {
    let pixels = [Rgba8::new(1, 2, 3, 4), Rgba8::new(0xfe, 0xdc, 0xba, 0x98)];
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry
        .begin_candidate(key_at(FIXTURE_START, 2, 1, ColorTargetFormat::Rgba32))
        .unwrap();
    let packed = pack_device_pixels(&candidate, &pixels).unwrap();
    assert_eq!(packed.device_bytes(), [1, 2, 3, 4, 0xfe, 0xdc, 0xba, 0x98]);
    assert_eq!(
        &*unpack_device_pixels(ColorTargetFormat::Rgba32, packed.device_bytes()).unwrap(),
        &pixels
    );
    assert!(matches!(
        packed.into_m3_3a_rgba16(),
        Err(TargetError::DeviceDomainMismatch { .. })
    ));
}

#[test]
fn hostile_target_length_and_address_overflow_are_loud() {
    let extent_error = ColorTargetExtent::try_new(u32::MAX, 2).unwrap_err();
    assert!(matches!(extent_error, TargetError::ExtentOverflow { .. }));

    let hardware_layout = PhysicalMemoryLayout::try_new(0x0100_0000).unwrap();
    let address = hardware_layout.address(0x00ff_fffc).unwrap();
    let error = ColorTargetKey::try_new(
        address,
        ColorTargetExtent::try_new(2, 1).unwrap(),
        ColorTargetFormat::Rgba32,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TargetError::Address(ValidationError::RangeOutOfBounds { .. })
    ));
}

#[test]
fn hostile_rectangle_overflow_and_bounds_are_loud() {
    assert!(matches!(
        TargetRectangle::try_new(u32::MAX, 0, 1, 1),
        Err(TargetError::RectangleCoordinateOverflow { axis: "x", .. })
    ));
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let outside = TargetRectangle::try_new(3, 1, 2, 1).unwrap();
    assert!(matches!(
        candidate.plan_rows(outside),
        Err(TargetError::RectangleOutOfBounds { .. })
    ));
}

#[test]
fn incompatible_overlap_is_rejected_without_mutating_residents() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let first = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    registry
        .commit_initialized(initialized(&registry, first))
        .unwrap();

    let alias = key_at(FIXTURE_START + 8, 2, 2, ColorTargetFormat::Rgba16);
    let error = registry.begin_candidate(alias).unwrap_err();
    assert!(matches!(error, TargetError::AliasedResidentTarget { .. }));
    assert_eq!(registry.residents().len(), 1);
    assert_eq!(registry.residents()[0].key(), first);
}

#[test]
fn same_bytes_with_a_different_format_are_an_alias_not_a_reinterpretation() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let rgba16 = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    registry
        .commit_initialized(initialized(&registry, rgba16))
        .unwrap();
    let rgba32 = key_at(FIXTURE_START, 2, 2, ColorTargetFormat::Rgba32);
    assert_eq!(rgba16.range(), rgba32.range());
    assert!(matches!(
        registry.begin_candidate(rgba32),
        Err(TargetError::AliasedResidentTarget { .. })
    ));
}

#[test]
fn unsupported_rdp_formats_never_enter_target_ownership() {
    for (format, size) in [
        (ImageFormat::Yuv, PixelSize::Bits16),
        (ImageFormat::ColorIndex, PixelSize::Bits8),
        (ImageFormat::IntensityAlpha, PixelSize::Bits16),
        (ImageFormat::Intensity, PixelSize::Bits8),
        (ImageFormat::Rgba, PixelSize::Bits4),
        (ImageFormat::Rgba, PixelSize::Bits8),
    ] {
        assert!(matches!(
            ColorTargetFormat::try_from_rdp(format, size),
            Err(TargetError::UnsupportedColorTargetFormat { .. })
        ));
    }
}

#[test]
fn a_partial_new_target_completion_is_admitted_and_names_what_it_covered() {
    // **Retargeted, not deleted.** This used to assert
    // `PartialNewTargetInitialization` -- that a brand-new target could not
    // become resident from a partial rectangle. That refusal is gone: the
    // pixels outside the rectangle now come from the guest's own
    // framebuffer (see `targets/fill.rs` and
    // `docs/RT64-FILL-PARTIAL-SEED.md`), so a partial completion is
    // ordinary rather than unrepresentable.
    //
    // What replaces it is the fact a later reader actually needs, and which
    // the old `rows` count could not express: the proof must name WHICH
    // rectangle this generation covered. `rows` is a height with no origin,
    // so it cannot distinguish "row 0" from "row 1"; `covered` can, and
    // this fixture uses a rectangle at a nonzero origin so the two answers
    // genuinely differ.
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    let rectangle = TargetRectangle::try_new(0, 1, 4, 1).unwrap();
    let partial = candidate.plan_rows(rectangle).unwrap();
    let completion = completed(&candidate, partial, &[Rgba8::new(255, 0, 0, 255); 8]);
    let initialized = candidate
        .admit_completed_initialization(completion)
        .unwrap();
    assert_eq!(
        initialized.initialized_region().covered(),
        rectangle,
        "the proof must name the covered rectangle, origin included"
    );
    assert!(
        !initialized.initialized_region().is_full(key.extent()),
        "a one-row completion of a two-row target is not full"
    );
}

#[test]
fn partial_resident_update_is_admitted_when_the_byte_buffer_still_covers_the_full_target() {
    // A resident (predecessor.is_some()) candidate may complete a partial
    // rectangle, unlike a brand-new target (see
    // partial_new_target_rejection_is_loud_and_non_publishing above) --
    // provided its DeviceColorBytes buffer is still full-extent-sized (the
    // byte-length check earlier in admit_completed_initialization already
    // enforces that). See targets/fill.rs's execute_fill_rectangle for the
    // real read-modify-write producer of such a buffer; this test exercises
    // the lower-level admission API directly.
    let mut registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    registry
        .commit_initialized(initialized(&registry, key))
        .unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let partial = candidate
        .plan_rows(TargetRectangle::try_new(0, 1, 4, 1).unwrap())
        .unwrap();
    let completion = completed(&candidate, partial, &[Rgba8::new(0, 255, 0, 255); 8]);
    let initialized = candidate
        .admit_completed_initialization(completion)
        .unwrap();
    assert_eq!(initialized.initialized_region().rows(), 1);
    let resident = registry.commit_initialized(initialized).unwrap();
    assert_eq!(resident.generation(), TargetGeneration(2));
    assert_eq!(
        resident.device_bytes().device_bytes(),
        [0x07, 0xC1].repeat(8) // full 8-pixel buffer, all green (from `completed`'s fixed pixels)
    );
}

#[test]
fn device_byte_domain_rejects_short_and_long_pixel_sets() {
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(key).unwrap();
    for actual in [7, 9] {
        assert!(matches!(
            pack_device_pixels(
                &candidate,
                &vec![Rgba8::new(255, 0, 0, 255); actual]
            ),
            Err(TargetError::PixelCountMismatch {
                expected: 8,
                actual: observed,
                ..
            }) if observed == actual
        ));
    }
}

#[test]
fn forged_completion_key_generation_and_range_are_rejected() {
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let other = key_at(0x800, 4, 2, ColorTargetFormat::Rgba16);

    for corruption in 0..3 {
        let candidate = registry.begin_candidate(key).unwrap();
        let plan = candidate.plan_rows(full_rectangle(key)).unwrap();
        let mut completion = completed(&candidate, plan, &[Rgba8::new(255, 0, 0, 255); 8]);
        match corruption {
            0 => completion.key = other,
            1 => completion.generation = TargetGeneration(2),
            2 => completion.range = other.range(),
            _ => unreachable!(),
        }
        assert!(matches!(
            candidate.admit_completed_initialization(completion),
            Err(TargetError::InitializationPlanMismatch { .. })
        ));
    }
}

#[test]
fn forged_completed_byte_format_binding_and_lengths_are_rejected() {
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let other = key_at(0x800, 4, 2, ColorTargetFormat::Rgba16);

    for corruption in 0..5 {
        let candidate = registry.begin_candidate(key).unwrap();
        let plan = candidate.plan_rows(full_rectangle(key)).unwrap();
        let mut completion = completed(&candidate, plan, &[Rgba8::new(255, 0, 0, 255); 8]);
        match corruption {
            0 => completion.device_bytes.key = other,
            1 => completion.device_bytes.generation = TargetGeneration(2),
            2 => completion.device_bytes.format = ColorTargetFormat::Rgba32,
            3 => completion.device_bytes.bytes = vec![0; 15].into_boxed_slice(),
            4 => completion.device_bytes.bytes = vec![0; 17].into_boxed_slice(),
            _ => unreachable!(),
        }
        let result = candidate.admit_completed_initialization(completion);
        if corruption < 3 {
            assert!(matches!(
                result,
                Err(TargetError::CompletedByteDomainMismatch { .. })
            ));
        } else {
            assert!(matches!(
                result,
                Err(TargetError::CompletedByteLengthMismatch { expected: 16, .. })
            ));
        }
    }
}

#[test]
fn candidate_generation_promotes_once_and_stale_peer_is_rejected() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let first = initialized(&registry, key);
    assert_eq!(
        registry
            .commit_initialized(first)
            .unwrap()
            .generation()
            .get(),
        1
    );

    let candidate_a = registry.begin_candidate(key).unwrap();
    let candidate_b = registry.begin_candidate(key).unwrap();
    assert_eq!(candidate_a.generation().get(), 2);
    assert_eq!(candidate_b.generation().get(), 2);
    let plan_a = candidate_a.plan_rows(full_rectangle(key)).unwrap();
    let plan_b = candidate_b.plan_rows(full_rectangle(key)).unwrap();
    let pixels = [Rgba8::new(255, 0, 0, 255); 8];
    let completion_a = completed(&candidate_a, plan_a, &pixels);
    let completion_b = completed(&candidate_b, plan_b, &pixels);
    let initialized_a = candidate_a
        .admit_completed_initialization(completion_a)
        .unwrap();
    let initialized_b = candidate_b
        .admit_completed_initialization(completion_b)
        .unwrap();
    assert_eq!(
        registry
            .commit_initialized(initialized_a)
            .unwrap()
            .generation()
            .get(),
        2
    );
    assert!(matches!(
        registry.commit_initialized(initialized_b),
        Err(TargetError::StaleCandidateGeneration {
            expected_predecessor: Some(TargetGeneration(1)),
            actual_resident: Some(TargetGeneration(2)),
            ..
        })
    ));
    assert_eq!(registry.residents()[0].generation().get(), 2);
}

#[test]
fn exhausted_generation_rejects_a_successor_candidate() {
    let key = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let proof = InitializedRegionProof {
        covered: TargetRectangle::try_new(0, 0, 1, 1).unwrap(),
        range: key.range(),
        rows: key.extent().height(),
        generation: TargetGeneration(u64::MAX),
    };
    let registry = ColorTargetRegistry {
        layout: layout(),
        capacity: NonZeroUsize::new(1).unwrap(),
        residents: vec![ResidentColorTarget {
            key,
            generation: TargetGeneration(u64::MAX),
            initialized: proof,
            device_bytes: DeviceColorBytes {
                key,
                generation: TargetGeneration(u64::MAX),
                format: ColorTargetFormat::Rgba16,
                bytes: vec![0; key.range().len() as usize].into_boxed_slice(),
            },
        }],
    };
    assert!(matches!(
        registry.begin_candidate(key),
        Err(TargetError::GenerationExhausted {
            current: TargetGeneration(u64::MAX),
            ..
        })
    ));
}

#[test]
fn independently_planned_new_aliases_cannot_both_commit() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let first = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    let overlapping = key_at(FIXTURE_START + 8, 2, 2, ColorTargetFormat::Rgba16);
    let first_initialized = initialized(&registry, first);
    let overlapping_initialized = initialized(&registry, overlapping);
    registry.commit_initialized(first_initialized).unwrap();
    assert!(matches!(
        registry.commit_initialized(overlapping_initialized),
        Err(TargetError::AliasedResidentTarget { .. })
    ));
    assert_eq!(registry.residents().len(), 1);
}

#[test]
fn initialization_plan_cannot_cross_target_or_generation() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let first = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    registry
        .commit_initialized(initialized(&registry, first))
        .unwrap();
    let second = key_at(0x800, 4, 2, ColorTargetFormat::Rgba16);

    let first_candidate = registry.begin_candidate(first).unwrap();
    let second_candidate = registry.begin_candidate(second).unwrap();
    let alien_plan = second_candidate.plan_rows(full_rectangle(second)).unwrap();
    let alien_completion = completed(
        &second_candidate,
        alien_plan,
        &[Rgba8::new(255, 0, 0, 255); 8],
    );
    assert!(matches!(
        first_candidate.admit_completed_initialization(alien_completion),
        Err(TargetError::InitializationPlanMismatch { .. })
    ));
}

#[test]
fn m3_2_color_image_maps_without_losing_its_physical_layout() {
    let image = ColorImage::from_wire(
        ImageFormat::Rgba,
        PixelSize::Bits16,
        4,
        layout().address(FIXTURE_START).unwrap(),
    );
    let key = ColorTargetKey::try_from_color_image(image, 2).unwrap();
    assert_eq!(key.address(), image.address());
    assert_eq!(key.extent(), ColorTargetExtent::try_new(4, 2).unwrap());
    assert_eq!(key.format(), ColorTargetFormat::Rgba16);
    assert_eq!(
        (key.range().start().get(), key.range().end()),
        (0x400, 0x410)
    );
}

#[test]
fn malformed_device_byte_lengths_are_rejected() {
    assert!(matches!(
        unpack_device_pixels(ColorTargetFormat::Rgba16, &[0]),
        Err(TargetError::PixelByteLength {
            required_multiple: 2,
            ..
        })
    ));
    assert!(matches!(
        unpack_device_pixels(ColorTargetFormat::Rgba32, &[0, 1, 2]),
        Err(TargetError::PixelByteLength {
            required_multiple: 4,
            ..
        })
    ));
}

#[test]
fn registry_capacity_and_memory_layout_are_explicit() {
    assert!(matches!(
        ColorTargetRegistry::try_new(layout(), 0),
        Err(TargetError::ZeroRegistryCapacity)
    ));
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let other_layout = PhysicalMemoryLayout::try_new(4 * 1024 * 1024).unwrap();
    let key = ColorTargetKey::try_new(
        other_layout.address(FIXTURE_START).unwrap(),
        ColorTargetExtent::try_new(4, 2).unwrap(),
        ColorTargetFormat::Rgba16,
    )
    .unwrap();
    assert!(matches!(
        registry.begin_candidate(key),
        Err(TargetError::MemoryLayoutMismatch { .. })
    ));
}

#[test]
fn adjacent_targets_do_not_alias() {
    let mut registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let first = key_at(FIXTURE_START, 4, 2, ColorTargetFormat::Rgba16);
    registry
        .commit_initialized(initialized(&registry, first))
        .unwrap();
    let adjacent = key_at(first.range().end(), 4, 2, ColorTargetFormat::Rgba16);
    let candidate = registry.begin_candidate(adjacent).unwrap();
    assert_eq!(candidate.generation(), TargetGeneration::FIRST);
}
