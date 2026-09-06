use super::*;

/// The shape every case below perturbs by exactly one field: a
/// non-degenerate viewport whose texcoords recover integer S10.5
/// endpoints, so a failure names the perturbation and nothing else.
fn admitted_viewport() -> RectViewportPixels {
    RectViewportPixels {
        left: 4,
        top: 8,
        right: 20,
        bottom: 24,
    }
}

/// Positive control: the unperturbed shape is admitted, and the
/// recovered endpoints are the exact `value * 32.0` inverses. Without
/// this, a case below could pass because the fixture is broken rather
/// than because the guard fired.
#[test]
fn the_unperturbed_shape_is_admitted_and_recovers_its_endpoints() {
    let draw =
        TexrectDraw::try_from_viewport_and_texcoords(admitted_viewport(), [1.0, 2.0], [3.5, 4.25])
            .expect("the unperturbed fixture is admitted");
    assert_eq!(
        (draw.s_start, draw.t_start, draw.s_end, draw.t_end),
        (32, 64, 112, 136),
        "endpoints are the integer inverses of the /32.0 RT64 emitted"
    );
    // Reconciled against an independent derivation from the same
    // literals, per the port card's two-independent-ways rule.
    assert_eq!(
        (
            (1.0f32 * 32.0) as i16,
            (2.0f32 * 32.0) as i16,
            (3.5f32 * 32.0) as i16,
            (4.25f32 * 32.0) as i16
        ),
        (32, 64, 112, 136)
    );
    assert_eq!(
        (draw.left, draw.top, draw.right, draw.bottom),
        (4, 8, 20, 24)
    );
}

#[test]
fn flipped_axes_advance_s_by_row_and_t_by_column() {
    let viewport = RectViewportPixels {
        left: 0,
        top: 0,
        right: 4,
        bottom: 2,
    };
    let ordinary =
        TexrectDraw::try_from_viewport_and_texcoords(viewport, [0.0, 0.0], [2.0, 4.0]).unwrap();
    let flipped = ordinary.with_flipped_axes();

    assert_eq!(ordinary.coordinates_at(1, 0), (16, 0));
    assert_eq!(ordinary.coordinates_at(0, 1), (0, 64));
    assert_eq!(flipped.coordinates_at(1, 0), (0, 32));
    assert_eq!(flipped.coordinates_at(0, 1), (32, 0));
}

/// Kills the `viewport.left < 0` half of the `NegativeViewportOrigin`
/// guard. `left` alone is negative; every other field stays admitted,
/// so the `EmptyViewport` guard below it does **not** also fire and
/// this test cannot pass by way of the wrong refusal.
#[test]
fn a_negative_viewport_left_is_refused_by_name() {
    let viewport = RectViewportPixels {
        left: -1,
        ..admitted_viewport()
    };
    assert_eq!(
        TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
        Err(TexrectExecutionError::NegativeViewportOrigin { viewport })
    );
    // The successor guard would not have caught this one: the extent
    // stays strictly positive, so deleting `NegativeViewportOrigin`
    // admits the rectangle rather than rejecting it differently.
    assert!(viewport.right > viewport.left && viewport.bottom > viewport.top);
}

/// Kills the `viewport.top < 0` half. Held separate from `left` because
/// a mutant deleting only one disjunct survives a single-axis test.
#[test]
fn a_negative_viewport_top_is_refused_by_name() {
    let viewport = RectViewportPixels {
        top: -1,
        ..admitted_viewport()
    };
    assert_eq!(
        TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
        Err(TexrectExecutionError::NegativeViewportOrigin { viewport })
    );
    assert!(viewport.right > viewport.left && viewport.bottom > viewport.top);
}

/// Kills the `right <= left` half of `EmptyViewport`, at both the
/// zero-width (`==`) and reversed (`<`) boundaries -- `<=` mutated to
/// `<` survives a reversed-only test.
#[test]
fn a_zero_width_and_a_reversed_viewport_are_both_refused_by_name() {
    for right in [4, 3] {
        let viewport = RectViewportPixels {
            right,
            ..admitted_viewport()
        };
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
            Err(TexrectExecutionError::EmptyViewport { viewport }),
            "right={right} against left={}",
            viewport.left
        );
    }
}

/// Kills the `bottom <= top` half, same two boundaries.
#[test]
fn a_zero_height_and_a_reversed_viewport_are_both_refused_by_name() {
    for bottom in [8, 7] {
        let viewport = RectViewportPixels {
            bottom,
            ..admitted_viewport()
        };
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
            Err(TexrectExecutionError::EmptyViewport { viewport }),
            "bottom={bottom} against top={}",
            viewport.top
        );
    }
}

/// Kills `NonIntegralTexcoord` on all four texcoord slots.
///
/// `1.0 / 64.0` is exactly representable, so `value * 32.0` is exactly
/// `0.5` -- the refusal is on a genuinely non-integral product, not on
/// a float-rounding artifact this test invented. Deleting the guard
/// silently truncates `0.5 as i16` to `0`, which is the truncation
/// `AGENTS.md` bans; each slot is exercised separately because the
/// closure is invoked four times and a per-slot mutation survives a
/// single-slot test.
#[test]
fn a_non_integral_texcoord_is_refused_by_name_on_every_slot() {
    let fractional = 1.0f32 / 64.0;
    assert_eq!(
        fractional * 32.0,
        0.5,
        "the product is exactly non-integral"
    );
    assert_eq!(
        (fractional * 32.0) as i16,
        0,
        "and a deleted guard would silently truncate it to zero"
    );
    for (slot, axis) in [
        (0usize, TexrectAxis::S),
        (1, TexrectAxis::T),
        (2, TexrectAxis::S),
        (3, TexrectAxis::T),
    ] {
        let mut coords = [1.0f32, 2.0, 3.5, 4.25];
        coords[slot] = fractional;
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(
                admitted_viewport(),
                [coords[0], coords[1]],
                [coords[2], coords[3]],
            ),
            Err(TexrectExecutionError::NonIntegralTexcoord {
                axis,
                value: fractional
            }),
            "slot {slot}"
        );
    }
}

/// Pins the non-finite refusals, which the fractional case above does
/// not reach.
///
/// **`!scaled.is_finite()` is a proven-equivalent disjunct, not an
/// untested one, and this test does not claim to kill it.** Deleting it
/// leaves all nine cases here green, and that survivor is equivalent
/// rather than a reach failure: an exhaustive sweep of all 2^32 `f32`
/// bit patterns found **zero** values for which `!is_finite()` holds
/// and `fract() == 0.0`, because every non-finite `fract()` is NaN and
/// `NaN != 0.0`. So `fract() != 0.0` alone already refuses every
/// infinity and every NaN, and the `is_finite` conjunct is dead on
/// every reachable input -- kept as documentation of intent, and it is
/// what stops `f32::INFINITY as i16` from silently saturating to
/// `i16::MAX` if the `fract` term were ever changed.
#[test]
fn non_finite_texcoords_are_refused_by_name() {
    for value in [f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(
                admitted_viewport(),
                [value, 2.0],
                [3.5, 4.25],
            ),
            Err(TexrectExecutionError::NonIntegralTexcoord {
                axis: TexrectAxis::S,
                value
            }),
            "{value}"
        );
    }
    // NaN is refused too, but `PartialEq` over the payload cannot
    // assert it by equality -- match on the variant instead.
    assert!(matches!(
        TexrectDraw::try_from_viewport_and_texcoords(
            admitted_viewport(),
            [f32::NAN, 2.0],
            [3.5, 4.25],
        ),
        Err(TexrectExecutionError::NonIntegralTexcoord {
            axis: TexrectAxis::S,
            ..
        })
    ));
}

/// Kills `TexcoordOutOfRange` at both ends, one step outside the S10.5
/// range and integral at the scale -- so it passes the
/// `NonIntegralTexcoord` guard above it and can only be caught here.
///
/// The witnesses are derived, not guessed: `i16::MAX + 1 = 32768` and
/// `i16::MIN - 1 = -32769` scaled back down by 32. Both are exactly
/// representable in f32 (integers well inside the 24-bit range), so
/// the product is exactly the out-of-range integer.
#[test]
fn an_out_of_range_texcoord_is_refused_by_name_at_both_ends() {
    let above = (f32::from(i16::MAX) + 1.0) / 32.0;
    let below = (f32::from(i16::MIN) - 1.0) / 32.0;
    assert_eq!((above * 32.0, below * 32.0), (32768.0, -32769.0));
    assert_eq!(
        (above * 32.0).fract(),
        0.0,
        "integral, so the guard above does not fire first"
    );
    assert_eq!((below * 32.0).fract(), 0.0);
    for (value, axis, upper_left, lower_right) in [
        (above, TexrectAxis::S, [above, 2.0], [3.5, 4.25]),
        (below, TexrectAxis::T, [1.0, below], [3.5, 4.25]),
    ] {
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(
                admitted_viewport(),
                upper_left,
                lower_right,
            ),
            Err(TexrectExecutionError::TexcoordOutOfRange { axis, value }),
            "{value}"
        );
    }
    // The endpoints themselves stay admitted -- a mutant tightening
    // `<`/`>` into `<=`/`>=` is killed here, not by the cases above.
    let draw = TexrectDraw::try_from_viewport_and_texcoords(
        admitted_viewport(),
        [f32::from(i16::MIN) / 32.0, f32::from(i16::MAX) / 32.0],
        [3.5, 4.25],
    )
    .expect("the inclusive S10.5 endpoints are admitted");
    assert_eq!((draw.s_start, draw.t_start), (i16::MIN, i16::MAX));
}

/// Every refusal renders a non-empty message naming its own axis or
/// viewport, so a deleted guard cannot be replaced by a silent one.
#[test]
fn each_construction_refusal_renders_an_actionable_message() {
    let viewport = admitted_viewport();
    for (error, needle) in [
        (
            TexrectExecutionError::NegativeViewportOrigin { viewport },
            "negative",
        ),
        (TexrectExecutionError::EmptyViewport { viewport }, "empty"),
        (
            TexrectExecutionError::NonIntegralTexcoord {
                axis: TexrectAxis::S,
                value: 0.5,
            },
            "S",
        ),
        (
            TexrectExecutionError::TexcoordOutOfRange {
                axis: TexrectAxis::T,
                value: 4096.0,
            },
            "T",
        ),
    ] {
        let rendered = error.to_string();
        assert!(
            rendered.contains(needle),
            "{error:?} rendered {rendered:?} without {needle:?}"
        );
    }
}
