use super::*;

/// One advance committing exactly one field -- the ordinary case.
fn sample(wall_ms: f64, virtual_cycles: u64) -> FieldSample {
    FieldSample {
        wall_ms,
        fields: 1,
        virtual_cycles,
        gfx_submits: 0,
        counters: Counters::default(),
    }
}

/// Cumulative submit counts, as the census records them.
fn rendering_span(count: usize, wall_ms: f64) -> Vec<FieldSample> {
    (0..count)
        .map(|i| FieldSample {
            wall_ms,
            fields: 1,
            virtual_cycles: 0,
            gfx_submits: 1_000 + i as u64,
            counters: Counters::default(),
        })
        .collect()
}

#[test]
fn reports_nearest_rank_percentiles_and_counts_budget_breaches() {
    let samples: Vec<FieldSample> = (1..=100).map(|ms| sample(ms as f64, 0)).collect();
    let d = FrameDistribution::from_samples(&samples).expect("100 samples");
    assert_eq!(d.samples, 100);
    assert_eq!(d.p50_ms, 50.0);
    assert_eq!(d.p95_ms, 95.0);
    assert_eq!(d.p99_ms, 99.0);
    assert_eq!(d.max_ms, 100.0);
    // 17..=100 exceed 16.667 ms; 1..=16 do not.
    assert_eq!(d.over_budget, 84);
    assert!(!d.holds_60fps());
}

/// The worst-case bar cannot be read off p50 or p95 -- the whole reason
/// this reports a distribution rather than a mean.
#[test]
fn one_slow_field_in_fifty_fails_the_bar_while_p50_and_p95_stay_fast() {
    let mut samples: Vec<FieldSample> = (0..49).map(|_| sample(8.0, 0)).collect();
    samples.push(sample(45.0, 0));
    let d = FrameDistribution::from_samples(&samples).expect("50 samples");
    assert_eq!(d.p50_ms, 8.0);
    assert_eq!(d.p95_ms, 8.0, "p95 cannot see one field in fifty");
    assert_eq!(d.max_ms, 45.0);
    assert_eq!(d.over_budget, 1);
    assert!(!d.holds_60fps());
}

#[test]
fn a_span_entirely_inside_one_field_holds_the_bound() {
    let samples: Vec<FieldSample> = (0..50).map(|_| sample(16.0, 0)).collect();
    let d = FrameDistribution::from_samples(&samples).expect("50 samples");
    assert_eq!(d.over_budget, 0);
    assert!(d.holds_60fps(), "16.0 ms fits inside a 16.667 ms field");
}

/// The documented trap, pinned as a test: the SAME span reads as 2.4x on
/// the frame-budget ratio and 1.1x on wall-versus-virtual, because the
/// guest emitted fields at ~27 Hz rather than 60 Hz. A benchmark that
/// reported only one of these would be misleading in a specific,
/// previously-made way.
#[test]
fn the_two_ratios_diverge_when_the_guest_underproduces_fields() {
    // 100 fields, 40 ms of wall time each. The guest charged 37 ms of
    // virtual time per field (~27 Hz), not the nominal 16.667 ms.
    let virtual_cycles_per_field = (fn64_runtime::CPU_CLOCK_HZ as f64 * 0.037).round() as u64;
    let samples: Vec<FieldSample> = (0..100)
        .map(|_| sample(40.0, virtual_cycles_per_field))
        .collect();
    let d = FrameDistribution::from_samples(&samples).expect("100 samples");

    // Ratio A: against the 60fps budget.
    assert!((d.wall_ms_per_field() - 40.0).abs() < 1e-9);
    let budget_ratio = d.wall_ms_per_field() / FRAME_BUDGET_MS;
    assert!(
        (budget_ratio - 2.4).abs() < 0.01,
        "expected ~2.4x the frame budget, got {budget_ratio}"
    );

    // Ratio B: against the guest's own clock.
    let versus = d.wall_versus_virtual().expect("virtual time elapsed");
    assert!(
        (versus - 1.081).abs() < 0.01,
        "expected ~1.08x wall-versus-virtual, got {versus}"
    );

    // And the reason they differ is a measurable, reportable quantity.
    let hz = d.guest_field_hz().expect("virtual time elapsed");
    assert!((hz - 27.0).abs() < 0.1, "expected ~27Hz, got {hz}");

    assert!(
        budget_ratio > versus * 2.0,
        "the trap is that these two are far apart; if they were close, \
         quoting one for the other would be harmless"
    );
}

/// A ratio needs a denominator. Reporting `inf` (or a panic) for a span
/// where no virtual time elapsed would be worse than saying so.
#[test]
fn ratios_are_absent_rather_than_infinite_without_virtual_time() {
    let samples: Vec<FieldSample> = (0..10).map(|_| sample(5.0, 0)).collect();
    let d = FrameDistribution::from_samples(&samples).expect("10 samples");
    assert_eq!(d.wall_versus_virtual(), None);
    assert_eq!(d.guest_field_hz(), None);
    // The budget ratio is still well defined and still the bar.
    assert!((d.wall_ms_per_field() - 5.0).abs() < 1e-9);
}

#[test]
fn an_empty_span_has_no_distribution() {
    assert!(FrameDistribution::from_samples(&[]).is_none());
}

/// The exact scenario a peer agent attributed by counter (`8690d36`): a
/// 3,028 ms advance that committed 22 VI fields. Charging it to one frame
/// reports a three-second frame nobody experienced -- the guest stayed
/// runnable across those 22 deadlines, its clock advanced every one, and
/// the guest was running FASTER than average during the span.
///
/// Normalizing gives 137.6 ms/field, which is a real and reportable miss,
/// but 22x smaller than the artifact.
#[test]
fn a_multi_field_catch_up_is_normalized_rather_than_reported_as_one_huge_frame() {
    let mut samples: Vec<FieldSample> = (0..99).map(|_| sample(10.0, 0)).collect();
    samples.push(FieldSample {
        wall_ms: 3028.0,
        fields: 22,
        virtual_cycles: 0,
        gfx_submits: 0,
        counters: Counters::default(),
    });
    let d = FrameDistribution::from_samples(&samples).expect("100 advances");

    assert_eq!(d.samples, 100, "100 advances");
    assert_eq!(d.fields, 121, "99 single fields + 22 from the catch-up");
    assert!(
        (d.max_ms - 3028.0 / 22.0).abs() < 0.01,
        "max must be the PER-FIELD 137.6ms, not the raw 3028ms; got {}",
        d.max_ms
    );
    assert!(d.has_multi_field_advances());

    // The raw advance is still retained, so a real stall stays visible.
    assert_eq!(d.max_advance_ms, 3028.0);
    assert_eq!(d.max_advance_fields, 22);

    // And all 22 fields are counted as budget misses, not just one.
    assert_eq!(d.over_budget, 22);
}

/// Normalization must not launder an actual host stall. Same wall time,
/// one field: this one IS a one-and-a-half-second hitch and must read as
/// one.
#[test]
fn a_single_field_stall_is_not_normalized_away() {
    let mut samples: Vec<FieldSample> = (0..99).map(|_| sample(10.0, 0)).collect();
    samples.push(sample(1475.0, 0));
    let d = FrameDistribution::from_samples(&samples).expect("100 advances");

    assert_eq!(d.fields, 100, "no catch-up occurred");
    assert!(!d.has_multi_field_advances());
    assert_eq!(
        d.max_ms, 1475.0,
        "a one-field advance keeps its full wall time"
    );
    assert_eq!(d.over_budget, 1);
}

/// The mean is per FIELD, so it is directly comparable to 16.667 ms. A
/// per-advance mean would read high by the catch-up factor.
#[test]
fn the_mean_is_per_field_not_per_advance() {
    let samples = vec![
        sample(10.0, 0),
        FieldSample {
            wall_ms: 90.0,
            fields: 9,
            virtual_cycles: 0,
            gfx_submits: 0,
            counters: Counters::default(),
        },
    ];
    let d = FrameDistribution::from_samples(&samples).expect("2 advances");
    assert_eq!(d.fields, 10);
    assert_eq!(d.wall_ms, 100.0);
    assert!(
        (d.mean_ms - 10.0).abs() < 1e-9,
        "100ms over 10 FIELDS is 10ms/field, not 50ms/advance; got {}",
        d.mean_ms
    );
}

/// The warmup gate must not leak the transient into the first steady
/// sample. `observe_vi_fields` advances `last_boundary` on EVERY field,
/// including gated ones, so the first steady sample is measured from the
/// last transient field rather than from an earlier point. Had the gate
/// skipped the timestamp update, the first steady sample would absorb the
/// entire boot transient -- hundreds of milliseconds -- and would own
/// `max` for the rest of the run, which is precisely the startup-in-the-
/// p99 error the window exists to prevent.
#[test]
fn the_warmup_boundary_does_not_leak_transient_time_into_the_first_sample() {
    let mut census = Census::default();
    let base = Instant::now();

    // Simulate the observe path's bookkeeping across a gate opening.
    // Transient: one very slow field.
    census.last_boundary = Some(base);
    let transient_end = base + std::time::Duration::from_millis(500);
    let transient_ms = transient_end.duration_since(base).as_secs_f64() * 1000.0;
    census.transient_wall_ms += transient_ms;
    census.transient_fields += 1;
    census.last_boundary = Some(transient_end);

    // Steady: the next field takes 10 ms, measured from the transient's
    // END, not from `base`.
    let steady_end = transient_end + std::time::Duration::from_millis(10);
    let steady_ms = steady_end
        .duration_since(census.last_boundary.expect("set above"))
        .as_secs_f64()
        * 1000.0;
    census.samples.push(sample(steady_ms, 0));

    let d = FrameDistribution::from_samples(&census.samples).expect("1 sample");
    assert!(
        (d.max_ms - 10.0).abs() < 1.0,
        "first steady sample must be ~10ms, not ~510ms; got {}",
        d.max_ms
    );
    assert!(
        d.holds_60fps() || d.max_ms < 20.0,
        "the 500ms transient must not appear in the steady distribution"
    );
    assert!(
        (census.transient_wall_ms - 500.0).abs() < 1.0,
        "and the transient must still be REPORTED, not discarded silently"
    );
}

/// `gfx_submits` is cumulative per sample, so the span total is a
/// DIFFERENCE. Summing them would report 100,450 submits for a 100-field
/// span that rendered 99, and would make every idle span look busy.
#[test]
fn span_submits_are_a_difference_not_a_sum() {
    let d = FrameDistribution::from_samples(&rendering_span(100, 10.0)).expect("100 samples");
    assert_eq!(d.gfx_submits, 99, "1099 - 1000");
    assert!((d.gfx_per_field() - 0.99).abs() < 1e-9);
}

/// The failure mode this census exists to make visible: a span can post an
/// excellent latency distribution while rendering nothing at all, which is
/// exactly what the standard 19,523-step benchmark route does
/// (`gfx_submits=0`). Latency over an idle guest is not a frame time.
#[test]
fn an_idle_span_is_distinguishable_from_a_rendering_one_at_identical_latency() {
    let idle: Vec<FieldSample> = (0..100).map(|_| sample(4.0, 0)).collect();
    let rendering = rendering_span(100, 4.0);

    let idle = FrameDistribution::from_samples(&idle).expect("100 samples");
    let rendering = FrameDistribution::from_samples(&rendering).expect("100 samples");

    // Identical on every latency statistic, including the bar itself.
    assert_eq!(idle.p50_ms, rendering.p50_ms);
    assert_eq!(idle.max_ms, rendering.max_ms);
    assert!(idle.holds_60fps() && rendering.holds_60fps());

    // Separable only by the rendering evidence.
    assert_eq!(idle.gfx_submits, 0);
    assert_eq!(rendering.gfx_submits, 99);
}

/// A sample carrying a per-field counter delta.
fn sample_with(wall_ms: f64, counters: Counters) -> FieldSample {
    FieldSample {
        wall_ms,
        fields: 1,
        virtual_cycles: 0,
        gfx_submits: 0,
        counters,
    }
}

/// The partition is at the budget, and it is exclusive at the boundary
/// exactly as `over_budget` is -- a field of precisely 16.667 ms is fast,
/// because `holds_60fps` accepts it. Two definitions of "over budget" in
/// one module would let the split disagree with the headline count.
#[test]
fn the_split_partitions_at_the_same_boundary_the_over_budget_count_uses() {
    let mut samples: Vec<FieldSample> = (0..30).map(|_| sample(10.0, 0)).collect();
    samples.extend((0..70).map(|_| sample(40.0, 0)));
    samples.push(sample(FRAME_BUDGET_MS, 0));

    let d = FrameDistribution::from_samples(&samples).expect("101 samples");
    let split = PopulationSplit::from_samples(&samples, true).expect("101 samples");

    assert_eq!(split.slow.fields, 70);
    assert_eq!(split.fast.fields, 31, "the exactly-16.667ms field is FAST");
    assert_eq!(
        split.slow.fields as usize, d.over_budget,
        "the split's slow bucket must equal the headline over-budget count"
    );
}

/// The mechanism-naming case: slow fields carrying more graphics tasks.
/// The per-field ratio must show it, and must not be confused by the
/// buckets having different sizes.
#[test]
fn a_counter_that_differs_between_populations_reads_as_a_ratio_per_field() {
    let fast = Counters {
        gfx_tasks: 1,
        ..Counters::default()
    };
    let slow = Counters {
        gfx_tasks: 3,
        ..Counters::default()
    };
    // Deliberately unequal bucket sizes: 80 fast, 20 slow. A per-BUCKET
    // total would read 80 vs 60 and invert the finding.
    let mut samples: Vec<FieldSample> = (0..80).map(|_| sample_with(10.0, fast)).collect();
    samples.extend((0..20).map(|_| sample_with(40.0, slow)));

    let split = PopulationSplit::from_samples(&samples, true).expect("100 samples");
    assert_eq!(split.fast.counters.gfx_tasks, 80);
    assert_eq!(split.slow.counters.gfx_tasks, 60);

    let text = population_report(&split);
    assert!(
        text.contains("<== DIFFERS"),
        "a 3x per-field difference must be flagged; got:\n{text}"
    );
    assert!(
        text.contains("ratio=  3.000x"),
        "the ratio must be PER FIELD (3/1), not per bucket (60/80); got:\n{text}"
    );
    assert!(
        !text.contains("NO COUNTER DISTINGUISHES"),
        "a counter does distinguish them here"
    );
}

/// The bucket accumulator (`Bucket::from_samples`'s `add!`) is a
/// hand-maintained field list, and a counter omitted from it accumulates
/// to ZERO in both populations while `labelled()` dutifully prints it --
/// which reads as "this counter does not distinguish the populations",
/// the single most misleading output this module can produce. Adding the
/// executor-split counters hit exactly that: they were in the struct, the
/// sampler and the report, and silently absent from `add!`.
///
/// This pins it generically. Every field `labelled()` reports must survive
/// a round trip through `from_samples`, so a future counter cannot be
/// added to the struct and forgotten in the accumulator.
#[test]
fn every_labelled_counter_survives_bucket_accumulation() {
    // 1 in every field, so a surviving counter sums to the field count and
    // a dropped one sums to zero -- unambiguous either way.
    let ones = Counters {
        gfx_tasks: 1,
        audio_tasks: 1,
        executor_ns: 1,
        gfx_ns: 1,
        gfx_lle_ns: 1,
        gfx_lle_rsp_ns: 1,
        gfx_lle_rdp_ns: 1,
        vi_present_ns: 1,
        audio_lle_ns: 1,
        executor_calls: 1,
        gfx_calls: 1,
        gfx_lle_calls: 1,
        audio_lle_calls: 1,
        rsp_steps_gfx: 1,
        rsp_steps_audio: 1,
        rsp_entries: 1,
        dpc_calls: 1,
        barrier_served: 1,
        barrier_fell_back: 1,
        barrier_dirty_pages: 1,
        barrier_clean: 1,
        exec_mirror_ns: 1,
        exec_resume_ns: 1,
        exec_devtime_ns: 1,
        exec_guard_suspend_ns: 1,
        exec_guard_device_ns: 1,
        exec_mirror_calls: 1,
        exec_guard_suspend_calls: 1,
        exec_guard_device_calls: 1,
        resume_reconcile_ns: 1,
        resume_cop0_ns: 1,
        resume_dispatch_ns: 1,
        resume_invalidate_ns: 1,
        resume_exit_ns: 1,
        resume_suspend_ns: 1,
        resume_resolve_ns: 1,
        resume_hostcall_ns: 1,
        resume_hostcall_calls: 1,
        resume_reconcile_calls: 1,
        resume_dispatch_calls: 1,
        vi_present_in_executor_calls: 1,
        vi_present_outside_executor_calls: 1,
        dpc_alloc_ns: 1,
        dpc_copy_in_ns: 1,
        dpc_copy_back_ns: 1,
    };
    let samples: Vec<FieldSample> = (0..10).map(|_| sample_with(20.0, ones)).collect();
    let refs: Vec<&FieldSample> = samples.iter().collect();
    let bucket = Bucket::from_samples(&refs);
    for (label, value) in bucket.counters.labelled() {
        assert_eq!(
            value, 10,
            "`{label}` is in `labelled()` but did not accumulate through \
             `Bucket::from_samples`'s `add!` list -- it would read zero in BOTH \
             populations and be reported as 'does not distinguish them'"
        );
    }
}

/// Rule 6a: before trusting the split, confirm it can FAIL. An unarmed
/// run must say ABSENT, not print zeros -- a zero decomposition is
/// indistinguishable from "the apparatus costs nothing", which is the
/// exact wrong conclusion this instrument exists to test for.
#[test]
fn an_unarmed_executor_split_reports_absent_rather_than_zero() {
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, Counters::default()))
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = executor_split_report(&split.fast, &split.slow);
    assert!(
        text.contains("NOT ARMED"),
        "an unarmed split must say so; got:\n{text}"
    );
    assert!(
        !text.contains("APPARATUS"),
        "an unarmed split must not print an apparatus share; got:\n{text}"
    );
}

/// The arithmetic the whole section exists for: nested counters are
/// SUBTRACTED, never summed. Pinned with numbers where a summing bug is
/// visible -- resume 10ms containing mirror 4ms and guard 3ms leaves a NET
/// of 3ms, and an implementation that treated them as peers would report
/// 17ms of phases inside a 12ms executor, which is impossible.
#[test]
fn nested_executor_phases_are_subtracted_not_summed() {
    // Per field, in ns: executor 12ms = resume 10ms + devtime 1.5ms
    // + 0.5ms residual. Inside resume: mirror 4ms, guard@suspend 3ms.
    let counters = Counters {
        executor_ns: 12_000_000,
        executor_calls: 100,
        exec_resume_ns: 10_000_000,
        exec_devtime_ns: 1_500_000,
        exec_mirror_ns: 4_000_000,
        exec_mirror_calls: 100,
        exec_guard_suspend_ns: 3_000_000,
        exec_guard_suspend_calls: 400,
        exec_guard_device_ns: 500_000,
        exec_guard_device_calls: 10,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| {
            sample_with(
                if i % 2 == 0 { 40.0 } else { 10.0 },
                // Both populations carry identical per-field work here;
                // this test is about the arithmetic, not the split.
                counters,
            )
        })
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = executor_split_report(&split.fast, &split.slow);

    // resume NET = 10 - 4 - 3 = 3ms, NOT 10 and NOT 17.
    assert!(
        text.contains("resume NET") && text.contains("3.000ms/field"),
        "resume net must subtract the nested phases (10-4-3=3); got:\n{text}"
    );
    // APPARATUS = mirror 4 + guard@suspend 3 + guard@device 0.5 = 7.5ms.
    assert!(
        text.contains("7.500ms/field"),
        "apparatus must sum the three guard phases (4+3+0.5=7.5); got:\n{text}"
    );
    // Residual = 12 - 10 - 1.5 = 0.5ms, reported rather than dropped.
    assert!(
        text.contains("residual") && text.contains("0.500ms/field"),
        "the run_one_step residual must be reported (12-10-1.5=0.5); got:\n{text}"
    );
    assert!(
        text.contains("[nested]"),
        "nested rows must be marked so they are not read as peers; got:\n{text}"
    );
}

/// Rule 6a one level deeper: an unarmed `resume NET` split must say ABSENT
/// rather than print a decomposition of zeros. A zero here would read as
/// "translated guest code costs nothing", which is the most misleading
/// sentence this module could emit.
#[test]
fn an_unarmed_resume_split_reports_absent_rather_than_zero() {
    let split = PopulationSplit::from_samples(
        &(0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, Counters::default()))
            .collect::<Vec<_>>(),
        true,
    )
    .expect("40 samples");
    let text = resume_split_report(&split.fast, &split.slow);
    assert!(
        text.contains("NOT ARMED") && text.contains("absent, not zero"),
        "an unarmed resume split must distinguish absent from zero; got:\n{text}"
    );
    assert!(
        !text.contains("TRANSLATED GUEST CODE ="),
        "an unarmed split must not print a guest-code figure at all; got:\n{text}"
    );
}

/// The arithmetic the whole report rests on: `dispatch` is INCLUSIVE of
/// graphics and audio, so translated guest code is the subtraction, and
/// the seven phases must close against a re-derived `resume NET`.
#[test]
fn resume_split_subtracts_nested_graphics_and_closes_against_resume_net() {
    // resume NET = resume 20 - mirror 4 - guard@suspend 1 = 15ms.
    // Phases: reconcile 1 + cop0 2 + dispatch 3 + invalidate 0.5
    //       + exit 0.5 + resolve 0.4 + hostcall 7 = 14.4.
    // PARKED = 15 - 14.4 = 0.6.
    // gfx 6 + audio 1 nest inside HOSTCALL (7), not dispatch.
    // TRANSLATED GUEST CODE is dispatch itself = 3.
    let counters = Counters {
        executor_ns: 25_000_000,
        executor_calls: 100,
        exec_resume_ns: 20_000_000,
        exec_mirror_ns: 4_000_000,
        exec_guard_suspend_ns: 1_000_000,
        gfx_ns: 6_000_000,
        audio_lle_ns: 1_000_000,
        resume_reconcile_ns: 1_000_000,
        resume_cop0_ns: 2_000_000,
        resume_dispatch_ns: 3_000_000,
        resume_dispatch_calls: 100,
        resume_invalidate_ns: 500_000,
        resume_exit_ns: 500_000,
        resume_resolve_ns: 400_000,
        resume_hostcall_ns: 7_000_000,
        resume_hostcall_calls: 20,
        vi_present_outside_executor_calls: 7,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = resume_split_report(&split.fast, &split.slow);

    assert!(
        text.contains("resume NET=15.000ms/field"),
        "resume NET must be re-derived as 20-4-1=15, not inherited; got:\n{text}"
    );
    // The headline: dispatch IS translated guest code, 3ms.
    assert!(
        text.contains("TRANSLATED GUEST CODE = 3.000ms/field"),
        "dispatch is translated guest code directly; got:\n{text}"
    );
    // gfx+audio nest in HOSTCALL and must not exceed it -- the inversion
    // that a child exceeding its parent exposed on the real route.
    assert!(
        text.contains("host calls (OS shims)") && text.contains("7.000ms/field"),
        "the host-call bucket must be its own row; got:\n{text}"
    );
    // Closure: 15 - 14.4 = 0.6, named as the suspend gap rather than
    // absorbed into a phase or hidden.
    assert!(
        text.contains("PARKED (other threads ran)") && text.contains("0.600ms/field"),
        "parked time must be reported so the split's closure is visible; got:\n{text}"
    );
    // A POSITIVE gap is expected and must not be flagged as brokenness.
    assert!(
        !text.contains("NEGATIVE GAP"),
        "a positive suspend gap is the normal case, not an instrument fault; got:\n{text}"
    );
}

/// The check that caught the real defect, pinned so the fix cannot relax
/// it. Phases claiming MORE than their own parent contains is impossible,
/// and it is exactly what a timer spanning the coroutine suspend produced
/// (-697% on the first smoke run). A positive gap is normal; a negative one
/// means the instrument is lying and the phases must not be read.
#[test]
fn phases_exceeding_resume_net_are_reported_as_a_broken_instrument() {
    let counters = Counters {
        executor_ns: 25_000_000,
        executor_calls: 100,
        exec_resume_ns: 10_000_000,
        // 40ms of phases inside a 10ms parent: impossible.
        resume_dispatch_ns: 40_000_000,
        resume_dispatch_calls: 100,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = resume_split_report(&split.fast, &split.slow);
    assert!(
        text.contains("NEGATIVE GAP") && text.contains("instrument is broken"),
        "phases exceeding resume NET must be called an instrument fault, not a finding; \
         got:\n{text}"
    );
}

/// A decomposition that does not close must SAY so. Pre-registered at 5%:
/// the point of a stated tolerance is that a large residual becomes the
/// finding rather than being quietly presented as a set of phases.
#[test]
fn the_suspend_gap_is_always_printed_even_when_it_dominates() {
    // resume NET = 20ms with only 5ms on-stack: a 15ms suspend gap, 75% of
    // the parent. That is a legitimate outcome once the clock stops at the
    // switch -- but it must be VISIBLE, because an unnamed 75% is exactly
    // how the original 21.72 ms stayed hidden for a session. The report
    // must print it as a row and must not silently absorb it.
    let counters = Counters {
        executor_ns: 25_000_000,
        executor_calls: 100,
        exec_resume_ns: 20_000_000,
        resume_dispatch_ns: 5_000_000,
        resume_dispatch_calls: 100,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = resume_split_report(&split.fast, &split.slow);
    assert!(
        text.contains("PARKED (other threads ran)") && text.contains("15.000ms/field"),
        "a dominating suspend gap must still be printed as its own row; got:\n{text}"
    );
    // It is a gap, not an instrument fault -- do not cry wolf on it.
    assert!(
        !text.contains("NEGATIVE GAP"),
        "a positive gap is not brokenness; got:\n{text}"
    );
}

/// A mean-only split cannot tell a flat bucket from a spiky one, and the
/// owner's complaint is choppiness rather than slowness. This pins that
/// the spread rows actually distinguish the two: same MEAN, different
/// distribution, and the report must say so.
#[test]
fn the_spread_rows_distinguish_a_spiky_phase_from_a_flat_one_at_equal_mean() {
    // `dispatch` alternates 2ms / 18ms (mean 10, p95/p50 = 9x).
    // `cop0` is a flat 10ms every field (mean 10, p95/p50 = 1x).
    // A mean-only table would show these as identical.
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| {
            let spiky = if i % 2 == 0 { 2_000_000 } else { 18_000_000 };
            sample_with(
                40.0,
                Counters {
                    exec_resume_ns: 40_000_000,
                    resume_dispatch_ns: spiky,
                    resume_dispatch_calls: 1,
                    resume_cop0_ns: 10_000_000,
                    ..Counters::default()
                },
            )
        })
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = resume_split_report(&split.fast, &split.slow);

    // Both phases carry the same total, so the mean rows agree...
    assert!(
        text.contains("SPREAD dispatch (GUEST)"),
        "the spread rows must be emitted; got:\n{text}"
    );
    // ...and the spread rows must not.
    //
    // Read the SLOW bucket specifically. Every sample above is 40 ms, so
    // the 16.667 ms partition puts all forty in `slow` and leaves `fast`
    // empty -- and the report emits a row set per bucket, `fast` first. A
    // bare `find` takes the empty bucket's row and reads p50=0, p95=0,
    // which looks like a broken instrument rather than an unpopulated one.
    let row = |bucket: &str, label: &str| {
        let prefix = format!("{bucket}: SPREAD {label}");
        text.lines()
            .find(|l| l.contains(&prefix))
            .unwrap_or_else(|| panic!("no {prefix} row in:\n{text}"))
    };
    let spiky_line = row("slow", "dispatch (GUEST)");
    let flat_line = row("slow", "cop0");
    assert!(
        spiky_line.contains("9.0x"),
        "an alternating 2/18ms phase must report a 9x spread; got:\n{spiky_line}"
    );
    assert!(
        flat_line.contains("1.0x"),
        "a constant phase must report a 1x spread; got:\n{flat_line}"
    );
}

/// The VI reachability check must be able to REFUTE its own hypothesis.
/// A check that reports "confirmed" regardless of the state it inspects is
/// not a check (rule 6a), so both outcomes are pinned here.
#[test]
fn vi_reachability_reports_confirmation_and_refutation_distinctly() {
    let confirming = Counters {
        vi_present_outside_executor_calls: 12,
        ..Counters::default()
    };
    let refuting = Counters {
        vi_present_in_executor_calls: 3,
        vi_present_outside_executor_calls: 9,
        ..Counters::default()
    };
    let render = |counters| {
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        resume_split_report(&split.fast, &split.slow)
    };
    let confirmed = render(confirming);
    assert!(
        confirmed.contains("CONFIRMED BY OBSERVATION")
            && confirmed.contains("NOT nested in executor_ns"),
        "all-outside presentations must confirm the claim; got:\n{confirmed}"
    );
    let refuted = render(refuting);
    assert!(
        refuted.contains("REFUTED") && refuted.contains("retract it"),
        "any inside-executor presentation must REFUTE the claim, not soften it; \
         got:\n{refuted}"
    );
    // The two outcomes must be distinguishable, which is the property that
    // makes this a check rather than a restatement.
    assert_ne!(
        confirmed.contains("REFUTED"),
        refuted.contains("REFUTED"),
        "the reachability check must distinguish its two outcomes"
    );
}

/// The finding that must not be manufactured away: two populations 4x
/// apart in wall time doing identical measurable work. The report has to
/// say so in those words.
#[test]
fn identical_work_in_both_populations_reports_no_counter_distinguishes_them() {
    let work = Counters {
        gfx_tasks: 2,
        rsp_steps_gfx: 500,
        barrier_served: 700,
        ..Counters::default()
    };
    let mut samples: Vec<FieldSample> = (0..50).map(|_| sample_with(10.0, work)).collect();
    samples.extend((0..50).map(|_| sample_with(40.0, work)));

    let split = PopulationSplit::from_samples(&samples, true).expect("100 samples");
    let text = population_report(&split);
    assert!(
        text.contains("NO COUNTER DISTINGUISHES THE TWO POPULATIONS"),
        "got:\n{text}"
    );
    assert!(!text.contains("<== DIFFERS"), "got:\n{text}");
    // And the split itself is real: the wall times genuinely differ 4x.
    assert!((split.slow.mean_ms / split.fast.mean_ms - 4.0).abs() < 1e-9);
}

/// **Perf-method rule 6a**: a check that returns the same answer whatever
/// the state is not a check. With counter sampling off every counter is
/// zero in both buckets, which is indistinguishable from "the two
/// populations do identical work" -- the single most consequential finding
/// this instrument can report. The unarmed report must therefore say NO
/// DATA and must NOT say the populations are indistinguishable.
#[test]
fn an_unarmed_split_says_no_data_rather_than_no_difference() {
    let mut samples: Vec<FieldSample> = (0..50).map(|_| sample(10.0, 0)).collect();
    samples.extend((0..50).map(|_| sample(40.0, 0)));

    let unarmed =
        population_report(&PopulationSplit::from_samples(&samples, false).expect("100 samples"));
    assert!(unarmed.contains("counters NOT SAMPLED"), "got:\n{unarmed}");
    assert!(
        !unarmed.contains("NO COUNTER DISTINGUISHES"),
        "an unarmed run must not be able to produce the headline negative finding; \
         got:\n{unarmed}"
    );

    // The ten-second test: run the check against the state it must reject.
    // Armed, over the SAME samples, it reaches the opposite verdict --
    // so the label is a function of the observation, not a constant.
    let armed =
        population_report(&PopulationSplit::from_samples(&samples, true).expect("100 samples"));
    assert!(armed.contains("NO COUNTER DISTINGUISHES"), "got:\n{armed}");
    assert!(!armed.contains("counters NOT SAMPLED"), "got:\n{armed}");
}

/// Both buckets keep their own latency statistics, and the slow bucket's
/// p50 is the number that says how far the slow population actually is
/// from the bar -- the whole-span p50 (which already fits) cannot say it.
#[test]
fn each_bucket_reports_its_own_distribution() {
    let mut samples: Vec<FieldSample> = (0..50).map(|_| sample(12.0, 0)).collect();
    samples.extend((0..50).map(|_| sample(39.0, 0)));

    let d = FrameDistribution::from_samples(&samples).expect("100 samples");
    let split = PopulationSplit::from_samples(&samples, true).expect("100 samples");

    assert!(d.p50_ms <= FRAME_BUDGET_MS, "the whole-span p50 fits");
    assert_eq!(split.fast.p50_ms, 12.0);
    assert_eq!(split.slow.p50_ms, 39.0, "and the slow half is 2.3x the bar");
}

/// Counter deltas are SUMS over a bucket, unlike `gfx_submits`, which is a
/// cumulative snapshot and must be differenced. Getting these two
/// reductions the wrong way round is the error
/// `span_submits_are_a_difference_not_a_sum` pins for the other one.
#[test]
fn bucket_counters_sum_per_field_deltas_rather_than_differencing_snapshots() {
    let delta = Counters {
        rsp_steps_audio: 7,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..10).map(|_| sample_with(40.0, delta)).collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("10 samples");
    assert_eq!(split.slow.counters.rsp_steps_audio, 70, "10 fields x 7");
    assert_eq!(split.fast.advances, 0);
}

/// A monotone counter cannot decrease, so an apparent decrease is a bug
/// elsewhere and must read as zero work -- never as `u64::MAX` work, which
/// would poison every aggregate downstream of it.
#[test]
fn a_counter_going_backwards_saturates_to_zero_rather_than_wrapping() {
    let later = Counters {
        gfx_tasks: 5,
        ..Counters::default()
    };
    let earlier = Counters {
        gfx_tasks: 9,
        ..Counters::default()
    };
    assert_eq!(later.delta(&earlier).gfx_tasks, 0);
}

/// Every counter the sampler collects must appear in the report. A field
/// added to `Counters` and forgotten in `labelled` would be invisible --
/// and an invisible counter is indistinguishable from one that does not
/// differ, which is the exact confusion this instrument exists to avoid.
#[test]
fn every_counter_field_is_labelled_for_the_report() {
    // Distinct nonzero values, so a duplicated or omitted field shows up
    // as a wrong sum rather than as a coincidental match.
    let counters = Counters {
        gfx_tasks: 1,
        audio_tasks: 2,
        executor_ns: 4,
        gfx_ns: 8,
        gfx_lle_ns: 16,
        gfx_lle_rsp_ns: 32,
        gfx_lle_rdp_ns: 64,
        vi_present_ns: 128,
        audio_lle_ns: 256,
        executor_calls: 512,
        gfx_calls: 1024,
        gfx_lle_calls: 2048,
        audio_lle_calls: 4096,
        rsp_steps_gfx: 8192,
        rsp_steps_audio: 16384,
        rsp_entries: 32768,
        dpc_calls: 65536,
        barrier_served: 131_072,
        barrier_fell_back: 262_144,
        barrier_dirty_pages: 524_288,
        barrier_clean: 1_048_576,
        exec_mirror_ns: 1 << 21,
        exec_resume_ns: 1 << 22,
        exec_devtime_ns: 1 << 23,
        exec_guard_suspend_ns: 1 << 24,
        exec_guard_device_ns: 1 << 25,
        exec_mirror_calls: 1 << 26,
        exec_guard_suspend_calls: 1 << 27,
        exec_guard_device_calls: 1 << 28,
        resume_reconcile_ns: 1 << 29,
        resume_cop0_ns: 1 << 30,
        resume_dispatch_ns: 1 << 31,
        resume_invalidate_ns: 1 << 32,
        resume_exit_ns: 1 << 33,
        resume_suspend_ns: 1 << 34,
        resume_resolve_ns: 1 << 35,
        resume_hostcall_ns: 1 << 40,
        resume_hostcall_calls: 1 << 41,
        resume_reconcile_calls: 1 << 36,
        resume_dispatch_calls: 1 << 37,
        vi_present_in_executor_calls: 1 << 38,
        vi_present_outside_executor_calls: 1 << 39,
        dpc_alloc_ns: 1 << 42,
        dpc_copy_in_ns: 1 << 43,
        dpc_copy_back_ns: 1 << 44,
    };
    let labelled = counters.labelled();
    let sum: u64 = labelled.iter().map(|&(_, v)| v).sum();
    assert_eq!(
        sum,
        (1u64 << 45) - 1,
        "each distinct power of two must appear exactly once in `labelled`"
    );
    let mut names: Vec<&str> = labelled.iter().map(|&(n, _)| n).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "labels must be unique");
}

/// **The decision rule, pinned before any real data existed.**
///
/// Three candidate shapes for a ~50/50 split give three autocorrelations
/// of opposite sign through this exact estimator. Recording the synthetic
/// values as a test means the rule cannot be quietly restated once the
/// measurement lands -- if a future change to `autocorrelation` moved
/// these, the verdict text would be reinterpreting a different statistic
/// under the same name.
#[test]
fn the_three_candidate_shapes_give_autocorrelations_of_opposite_sign() {
    let alternating: Vec<f64> = (0..1000)
        .map(|i| if i % 2 == 0 { 12.0 } else { 33.0 })
        .collect();
    let blocks: Vec<f64> = (0..1000)
        .map(|i| if (i / 100) % 2 == 0 { 12.0 } else { 33.0 })
        .collect();
    // A fixed 50/50 shuffle, not an RNG: the test must be deterministic.
    let pseudo_random: Vec<f64> = (0..1000)
        .map(|i: u64| {
            let mut h = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 32;
            if h % 2 == 0 {
                12.0
            } else {
                33.0
            }
        })
        .collect();

    let alt = autocorrelation(&alternating, 1);
    let blk = autocorrelation(&blocks, 1);
    let rnd = autocorrelation(&pseudo_random, 1);

    assert!(alt < -0.9, "strict alternation must read ~-1.0, got {alt}");
    assert!(blk > 0.9, "contiguous blocks must read ~+1.0, got {blk}");
    assert!(
        rnd.abs() < 0.3,
        "an unstructured 50/50 sequence must read near zero, got {rnd}"
    );
    // And the three verdicts they select are genuinely different, which is
    // what makes this a check rather than three numbers.
    for (value, expect) in [
        (alt, "ALTERNATION"),
        (blk, "CONTIGUOUS BLOCKS"),
        (rnd, "NEITHER"),
    ] {
        let p = Periodicity {
            lag1: value,
            lags: [0.0; 6],
            samples: 1000,
            no_submit_fast: 0,
            no_submit_slow: 0,
            submit_fast: 0,
            submit_slow: 0,
            submits_per_submitting_fast: 0.0,
            submits_per_submitting_slow: 0.0,
        };
        let text = periodicity_report(&p);
        assert!(
            text.contains(expect),
            "lag1={value} must select {expect}; got:\n{text}"
        );
    }
}

/// Lag-1 is blind to periods other than 2, and the report must not be
/// read as if it were not. A period-4 sequence is genuinely periodic and
/// lag-1 places it in the "NEITHER" band -- which is why the raw pattern
/// string is printed first and the higher lags are reported beside it.
#[test]
fn lag_one_cannot_see_a_period_four_cycle_but_lag_four_can() {
    let period_four: Vec<f64> = (0..1000)
        .map(|i| if (i / 2) % 2 == 0 { 12.0 } else { 33.0 })
        .collect();
    let lag1 = autocorrelation(&period_four, 1);
    let lag4 = autocorrelation(&period_four, 4);
    assert!(
        lag1.abs() < 0.3,
        "a period-4 cycle lands in the NEITHER band at lag 1, got {lag1}"
    );
    assert!(
        lag4 > 0.9,
        "but lag 4 sees it clearly, which is why lags 2..=6 are reported; got {lag4}"
    );
}

/// A constant sequence has zero variance. The estimator must say "no
/// variation" rather than emit a `NaN`, which would render as a broken
/// instrument and could be misread as a missing measurement.
#[test]
fn a_constant_sequence_autocorrelates_to_zero_rather_than_nan() {
    let flat = vec![16.0; 100];
    let value = autocorrelation(&flat, 1);
    assert!(value.is_finite(), "got {value}");
    assert_eq!(value, 0.0);
}

/// The contingency verdict must be a function of the table, not a
/// constant printed beside it -- rule 15's second half. Two tables, two
/// opposite conclusions, from the same code path.
#[test]
fn the_contingency_verdict_follows_the_table_in_both_directions() {
    let explains = Periodicity {
        lag1: -0.9,
        lags: [0.0; 6],
        samples: 1000,
        // Slow fields nearly all render, fast fields nearly none.
        no_submit_fast: 490,
        no_submit_slow: 10,
        submit_fast: 10,
        submit_slow: 490,
        submits_per_submitting_fast: 1.0,
        submits_per_submitting_slow: 2.9,
    };
    let text = periodicity_report(&explains);
    assert!(text.contains("submit count EXPLAINS the split"), "{text}");

    let does_not = Periodicity {
        // Both populations render at the same rate.
        no_submit_fast: 250,
        no_submit_slow: 250,
        submit_fast: 250,
        submit_slow: 250,
        ..explains
    };
    let text = periodicity_report(&does_not);
    assert!(
        text.contains("Submits do NOT explain the split"),
        "the same code must reach the opposite verdict on the opposite \
         table; got:\n{text}"
    );
}

/// The gate must agree with `write_barrier`'s: an empty value, `0`, and an
/// absent variable all mean off, so no spelling of "off" reads as on.
#[test]
fn only_affirmative_spellings_arm_the_census() {
    let name = "FN64_FRAME_CENSUS_TEST_GATE";
    for off in ["", "0", "no", "off", "false", "  "] {
        std::env::set_var(name, off);
        assert!(!env_flag(name), "{off:?} must read as off");
    }
    for on in ["1", "true", "yes", "on", " ON "] {
        std::env::set_var(name, on);
        assert!(env_flag(name), "{on:?} must read as on");
    }
    std::env::remove_var(name);
    assert!(!env_flag(name), "an absent variable must read as off");
}

// ---- FN64_PROFILE composition ------------------------------------
//
// These exercise `profile_report` directly rather than through the env
// gate: `profile::enabled()` memoizes a `OnceLock`, so a test that set the
// variable would leak into every other test in the binary and would be
// order-dependent. Calling the formatter with known counters tests the
// thing that can actually be wrong.

/// The counters behind the recorded acceptance table, as ns totals over
/// one field. Slow population, 1.5M-step route.
fn acceptance_counters() -> Counters {
    Counters {
        // The recorded slow field is 56.23 ms and `resume NET` is 45.687 ms
        // INSIDE it -- 83.2% (perf-method.md:2725-2727, :3064). An earlier
        // draft of this fixture used 45.687 as the FIELD, which made
        // `exec_resume` 55 ms inside a 45.687 ms field: impossible, and the
        // outermost check correctly rejected it. The fixture was wrong,
        // not the check -- which is the check earning its place.
        executor_ns: 56_230_000,
        executor_calls: 100,
        exec_resume_ns: 55_000_000,
        exec_mirror_ns: 8_848_000,
        exec_guard_suspend_ns: 465_000,
        resume_dispatch_ns: 9_528_000,
        resume_dispatch_calls: 100,
        resume_hostcall_ns: 32_700_000,
        gfx_ns: 32_119_000,
        gfx_lle_ns: 32_033_000,
        gfx_lle_rsp_ns: 5_637_000,
        gfx_lle_rdp_ns: 26_396_000,
        resume_reconcile_ns: 1_000_000,
        resume_cop0_ns: 900_000,
        resume_invalidate_ns: 500_000,
        resume_exit_ns: 400_000,
        resume_resolve_ns: 300_000,
        // Witnesses the DPC census channel; without it the report
        // correctly refuses, which is the check working.
        rsp_entries: 100,
        dpc_alloc_ns: 120_000,
        dpc_copy_in_ns: 900_000,
        dpc_copy_back_ns: 750_000,
        ..Counters::default()
    }
}

fn acceptance_split() -> PopulationSplit {
    let counters = acceptance_counters();
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 56.230 } else { 10.0 }, counters))
        .collect();
    PopulationSplit::from_samples(&samples, true).expect("40 samples")
}

/// The sequence channel is the one constituent with no counter to witness:
/// it is a request for N fields, so "did it arm" is "was a length asked
/// for". `sequence_dump_len` memoizes a `OnceLock`, so a test cannot set
/// the variable and observe an effect -- this arms it once for the whole
/// test binary, before any test reads it.
///
/// Without this the report correctly REFUSES in every profile test, which
/// is the check doing its job rather than a bug.
fn arm_sequence_channel_for_tests() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if !crate::diag_env::diag_env_present("FN64_FRAME_CENSUS_SEQUENCE") {
            // SAFETY: test-only, and `Once` serializes it against the
            // other profile tests that call this first.
            unsafe { std::env::set_var("FN64_FRAME_CENSUS_SEQUENCE", "400") };
        }
    });
    // Force the memoized read now, so it cannot be captured as 0 later.
    assert!(
        sequence_dump_len() > 0,
        "sequence channel must arm for these tests"
    );
}

/// EVERY ROW STATES BOTH DENOMINATORS. This is the fix for the single most
/// consequential error: "20.9% of resume NET" is what let guest code be
/// named the target, when "0.57x budget" alongside it would have shown
/// three modest-looking rows summing past the budget.
#[test]
fn the_profile_report_states_both_denominators_on_every_row() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    let guest = text
        .lines()
        .find(|l| l.contains("TRANSLATED GUEST CODE"))
        .expect("guest-code row present");
    assert!(guest.contains("9.528ms/field"), "{guest}");
    assert!(
        guest.contains("% of resume NET"),
        "share of parent: {guest}"
    );
    assert!(guest.contains("x budget"), "ratio to budget: {guest}");
}

/// The rows are SUMMED in code, against the denominator the decision uses.
/// Every individual number can be right while the sum contradicts the
/// conclusion drawn from them.
#[test]
fn the_profile_report_sums_the_rows_against_the_budget() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    assert!(
        text.contains("SUM OF THE ROWS ABOVE"),
        "the report must add the rows up, not leave it to the eye:\n{text}",
    );
}

/// Percentiles, never a bare mean: a mean has hidden the distribution
/// twice on this project.
#[test]
fn the_profile_report_gives_percentiles_per_population() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    for population in ["fast", "slow"] {
        let line = text
            .lines()
            .find(|l| l.contains(&format!("{population}: fields=")))
            .unwrap_or_else(|| panic!("{population} summary line present in:\n{text}"));
        for stat in ["p50=", "p95=", "p99="] {
            assert!(line.contains(stat), "{population} missing {stat}: {line}");
        }
    }
}

/// Provenance travels with the numbers. Reconstructing where a figure came
/// from cost the worst hours of the evening this was built in.
#[test]
fn the_profile_report_carries_its_own_provenance() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    assert!(text.contains("PROVENANCE"), "{text}");
    assert!(text.contains("binary:"), "{text}");
    assert!(text.contains("route:"), "{text}");
}

/// The scope legend is present at the point of reading, because six tags
/// carry `gfx_submits` with four different meanings.
#[test]
fn the_profile_report_disambiguates_colliding_names() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    assert!(text.contains("NAME SCOPES"), "{text}");
    assert!(text.contains("NOT a contradiction"), "{text}");
}

/// THE CHECK, end to end: a child exceeding its parent must refuse to
/// print that subtree rather than presenting it as a finding. This is the
/// `gfx_ns` 21.5 > parent 7.7 defect, which a human caught by eye.
#[test]
fn the_profile_report_refuses_a_subtree_whose_child_exceeds_its_parent() {
    arm_sequence_channel_for_tests();
    let counters = Counters {
        executor_ns: 40_000_000,
        executor_calls: 100,
        exec_resume_ns: 38_000_000,
        resume_dispatch_ns: 1_000_000,
        resume_dispatch_calls: 100,
        // 21.5ms of graphics inside a 7.7ms host-call parent: impossible.
        resume_hostcall_ns: 7_700_000,
        gfx_ns: 21_500_000,
        // Witness for the DPC channel: without it the report refuses on
        // an unarmed channel BEFORE reaching the tree check under test.
        rsp_entries: 100,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
        .collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = profile_report(&split);
    assert!(
        text.contains("TREE VIOLATION") && text.contains("REFUSING"),
        "a child exceeding its parent must refuse the subtree:\n{text}",
    );
    assert!(
        !text.contains("(of) RDP rasterization"),
        "the refused subtree's rows must NOT be printed:\n{text}",
    );
}

/// A healthy decomposition must NOT trip the refusal, or the check fires
/// always and means nothing (rule 6a).
#[test]
fn a_healthy_profile_report_prints_its_rows() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    println!("{text}");
    assert!(
        !text.contains("TREE VIOLATION"),
        "the acceptance counters must close:\n{text}",
    );
    assert!(text.contains("TRANSLATED GUEST CODE"), "{text}");
    assert!(text.contains("RDP rasterization"), "{text}");
}

/// An unarmed channel must refuse the whole report rather than present a
/// plausible subset, and must NAME the gate that failed.
#[test]
fn the_profile_report_refuses_when_a_channel_did_not_arm() {
    // Population counters never sampled: `armed: false`.
    let samples: Vec<FieldSample> = (0..40)
        .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, Counters::default()))
        .collect();
    let split = PopulationSplit::from_samples(&samples, false).expect("40 samples");
    let text = profile_report(&split);
    assert!(
        text.contains("REFUSING TO PRINT"),
        "a partial profile must be refused:\n{text}",
    );
    assert!(
        text.contains("FN64_FRAME_CENSUS_POPULATIONS"),
        "the refusal must name the missing gate:\n{text}",
    );
    assert!(
        !text.contains("TRANSLATED GUEST CODE"),
        "no rows may be printed alongside a refusal:\n{text}",
    );
}

/// THE OUTERMOST CHECK, which the parent/child tree cannot make.
///
/// Found by reading this report's own first real output: the fast bucket
/// claimed `resume NET` = 45.687 ms inside a measured 10.000 ms field --
/// 2.74x the budget of phases inside a 0.60x field. Every parent/child
/// relation held, because the field's wall time is not a counter and so is
/// not in the tree. An impossible decomposition passed the check that
/// exists to catch impossible decompositions.
#[test]
fn a_decomposition_larger_than_its_own_field_is_caught() {
    arm_sequence_channel_for_tests();
    // 45ms of phases inside a 10ms field: impossible, and invisible to
    // every parent/child test because the tree closes internally.
    let counters = Counters {
        executor_ns: 45_000_000,
        executor_calls: 100,
        exec_resume_ns: 45_000_000,
        resume_dispatch_ns: 45_000_000,
        resume_dispatch_calls: 100,
        rsp_entries: 100,
        ..Counters::default()
    };
    let samples: Vec<FieldSample> = (0..40).map(|_| sample_with(10.0, counters)).collect();
    let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
    let text = profile_report(&split);
    assert!(
        text.contains("DECOMPOSITION EXCEEDS ITS FIELD"),
        "phases larger than the field they sit in must be refused:\n{text}",
    );
    // And the tree itself must be silent here -- proving this check is
    // catching something the parent/child relations genuinely cannot.
    let ms = |ns: u64| ns as f64 / 1.0e6 / 40.0;
    let lookup = |c: &str| match c {
        "executor_ns" | "exec_resume_ns" => ms(counters.executor_ns * 40),
        "resume_dispatch_ns" => ms(counters.resume_dispatch_ns * 40),
        _ => 0.0,
    };
    assert!(
        crate::counter_tree::validate(&lookup).is_empty(),
        "premise of this test: the tree closes internally, so only the \
         field-level check can catch it",
    );
}

/// A decomposition that FITS its field must not trip the outermost check,
/// or it fires always and means nothing.
#[test]
fn a_decomposition_within_its_field_is_not_flagged() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    let slow_section = text
        .split("slow: fields=")
        .nth(1)
        .expect("slow section present");
    assert!(
        !slow_section.contains("DECOMPOSITION EXCEEDS ITS FIELD"),
        "45.687ms of resume NET inside a 56.230ms field must be accepted:\n{slow_section}",
    );
}

/// The mirror is a SIBLING of resume NET while being nested inside
/// `exec_resume_ns`. That single relationship is what prose made
/// confusable and three hand-written report sites got wrong.
#[test]
fn the_profile_report_places_the_mirror_beside_resume_net() {
    arm_sequence_channel_for_tests();
    let text = profile_report(&acceptance_split());
    let mirror = text
        .lines()
        .find(|l| l.contains("mirror boundary"))
        .expect("mirror row present");
    assert!(mirror.contains("sibling of NET"), "{mirror}");
    assert!(mirror.contains("8.848ms/field"), "{mirror}");
}
