//! The differential timing comparator: diff fn64's device-event stream against
//! a reference emulator's, both in the producer-neutral
//! [`crate::timing_trace`] schema (design spec
//! `docs/superpowers/specs/2026-07-23-timing-oracle-design.md`, component 3).
//!
//! This module is the acceptance gate for every timing-refinement item (U2 PI
//! latency, U5 AI drain / EEPROM / Flash busy, U6 RSP): a candidate timing
//! model is admitted only when its device-event stream matches the reference
//! within the documented tolerance. It never runs an emulator — it consumes two
//! already-ingested [`DeviceTraceIngest`](crate::timing_trace::DeviceTraceIngest)
//! streams (via [`ingest_jsonl`](crate::timing_trace::ingest_jsonl)) and reports
//! where they diverge.
//! Agreement additionally requires both envelopes to end with
//! [`DeviceTraceCompletion::Completed`](crate::timing_trace::DeviceTraceCompletion::Completed).
//! Matching aborted streams are failed evidence, including two empty aborted
//! streams; a matching event prefix cannot promote an unsuccessful capture.
//!
//! ## The approved tolerance philosophy (two-tier)
//!
//! Per the spec's open decision #2, approved: ORDERING is the hard gate; cycle
//! COUNTS get a per-device band, loose-then-tighten.
//!
//! 1. **Event ORDERING is ZERO-tolerance.** The sequence of `(event_kind,
//!    device)` pairs — with the `addr_or_source` payload, and `value_or_len`
//!    where it is meaningful — must match position-for-position. A missing,
//!    extra, or reordered event is a hard failure, reported at the first
//!    divergent aligned position with full context (what fn64 had vs what the
//!    reference had). This mirrors fn64's own `bytes → PI idle → MI pending →
//!    notify` fabric invariant, which is already zero-tolerance.
//!
//! 2. **Cycle COUNTS get a per-device tolerance BAND.** Once two events are
//!    aligned (same kind, same device, same payload, same occurrence order),
//!    their `cycle` stamps are compared; a difference beyond that device's band
//!    is a `CycleOutOfBand` divergence. The band is [`TimingTolerance`],
//!    configurable per device, defaulting generously (see
//!    [`TimingTolerance::initial_loose`]) because ordering is the hard gate and
//!    the cycle bands tighten as reference data accrues.
//!
//! ## `wrong == 0`: never report agreement it can't prove
//!
//! The comparator aligns the two streams strictly by position. It walks both in
//! lockstep; the FIRST position where the aligned pair disagrees on
//! `(event_kind, device, payload)` is an [`Divergence::Ordering`] and halts
//! alignment there — it does NOT resynchronize past a structural mismatch,
//! because any pairing past an unmatched event is a guess. A stream that is a
//! strict prefix of the other (one ran longer) diverges at the first unpaired
//! event: the shorter side is "missing" it. An unmatched event is a divergence,
//! never a silently skipped one.
//!
//! ## The SI fixed-window caveat
//!
//! fn64's fabric does not carry a per-request length for SI transfers — SI is
//! the fixed 64-byte PIF window, so the tap emits `value_or_len = 0` for SI
//! (see [`crate::timing_trace::capture`]). A reference producer reading a real
//! core's SI registers may report the true 64-byte length. So for
//! [`TimingDevice::Si`], the comparator does NOT compare `value_or_len`: it is
//! treated as a fixed window and excluded from the ordering/payload key. Every
//! other device compares its full payload.

use crate::timing_trace::{
    DeviceEvent, DeviceTraceCompletion, DeviceTraceIngest, TimingDevice, TimingDmaDirection,
    TimingEventKind, TimingPiDevice,
};

/// Per-device cycle tolerance band, in R4300 CPU master cycles. Two aligned events of the
/// same `(kind, device, payload)` whose cycle stamps differ by MORE than the
/// band's device entry are a [`Divergence::CycleOutOfBand`]; a difference at or
/// within the band is in-band and passes.
///
/// The bands are a judgment call (spec open decision #2). They start
/// **generous** — ordering is the hard gate; the cycle band exists so a
/// still-deterministic-policy model (fn64 today) is not spuriously flagged for
/// a small constant offset from a more silicon-accurate reference. Each band
/// tightens per device as reference data accrues and a measured model replaces
/// the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingTolerance {
    /// PI DMA start/complete band. PI is where the biggest fn64-vs-reference
    /// gap lives today: `FixedPiTiming` returns a constant latency independent
    /// of length, so a long transfer's completion can sit thousands of cycles
    /// off a length-proportional reference. Start generous.
    pub pi_cycles: u64,
    /// SI DMA band. SI is the fixed 64-byte PIF window (small, fixed transfer),
    /// so its timing spread is narrower than PI's, but the PIF/controller path
    /// still varies between cores — a moderate band.
    pub si_cycles: u64,
    /// AI DMA band. AI drain is computed from the DAC rate, principled but not
    /// a measured AI model; a moderate band until a measured model lands.
    pub ai_cycles: u64,
    /// MI interrupt raise/ack band. An MI edge is emitted at the same cycle as
    /// the device event that caused it, so once the causing DMA's cycle is in
    /// band the MI edge should be close; a tight-ish band.
    pub mi_cycles: u64,
    /// VI retrace band. VI retrace is a periodic field tick; its cadence is
    /// derived from the same clock on both sides, so it should track closely —
    /// the tightest default band.
    pub vi_cycles: u64,
}

impl TimingTolerance {
    /// The documented INITIAL LOOSE bands (spec open decision #2, approved
    /// philosophy: start loose, tighten as data accrues). These are starting
    /// values, not a silicon spec:
    ///
    /// - **PI: 4096 cycles.** `FixedPiTiming` is length-independent; a
    ///   length-proportional reference can be a few thousand cycles off on a
    ///   large transfer. Generous on purpose — this is exactly the gap U2's
    ///   measured PI model will close, and tightening this band is how U2 is
    ///   graded "done".
    /// - **SI: 2048 cycles.** Fixed 64-byte window; smaller spread than PI but
    ///   the controller/PIF path varies between cores.
    /// - **AI: 2048 cycles.** DAC-rate-derived drain vs a measured AI model.
    /// - **MI: 512 cycles.** An MI edge rides the cycle of its causing event;
    ///   once that event is in band the edge is close.
    /// - **VI: 256 cycles.** Periodic field tick off a shared clock; the
    ///   tightest default.
    ///
    /// As each device gets a measured model whose stream falls within a tighter
    /// band on the corpus, lower that device's value here — the band IS the
    /// per-device acceptance spec.
    pub const fn initial_loose() -> Self {
        Self {
            pi_cycles: 4096,
            si_cycles: 2048,
            ai_cycles: 2048,
            mi_cycles: 512,
            vi_cycles: 256,
        }
    }

    /// The band for a given device, in CPU master cycles.
    pub const fn band_for(&self, device: TimingDevice) -> u64 {
        match device {
            TimingDevice::Pi => self.pi_cycles,
            TimingDevice::Si => self.si_cycles,
            TimingDevice::Ai => self.ai_cycles,
            TimingDevice::Mi => self.mi_cycles,
            TimingDevice::Vi => self.vi_cycles,
            // SP and DP are captured by the fabric tap but have no dedicated
            // band field (the timing items in scope are PI/SI/AI/MI/VI). Reuse
            // the AI band as a moderate default so an unexpected SP/DP event is
            // still compared, never silently passed.
            TimingDevice::Sp | TimingDevice::Dp => self.ai_cycles,
        }
    }
}

impl Default for TimingTolerance {
    fn default() -> Self {
        Self::initial_loose()
    }
}

/// The identity of an aligned position that must match zero-tolerance: the
/// event kind, the device, the payload address/source, PI direction and typed
/// device-relative target, and — except for SI — the length. Two events with
/// the same key are order-compatible; the only remaining comparison is their
/// cycle stamps against the band.
///
/// SI's `value_or_len` is deliberately absent (the fixed-window caveat): fn64's
/// fabric emits 0 for it while a reference may emit the true 64, and that is not
/// a divergence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlignmentKey {
    event_kind: TimingEventKind,
    device: TimingDevice,
    addr_or_source: u32,
    /// `Some(len)` for every device except SI; `None` for SI (fixed window).
    value_or_len: Option<u32>,
    dma_direction: Option<TimingDmaDirection>,
    pi_device: Option<TimingPiDevice>,
    pi_offset: Option<u32>,
}

impl AlignmentKey {
    fn of(event: &DeviceEvent) -> Self {
        let value_or_len = match event.device {
            // SI fixed-window caveat: length is not part of SI's identity.
            TimingDevice::Si => None,
            _ => Some(event.value_or_len),
        };
        Self {
            event_kind: event.event_kind,
            device: event.device,
            addr_or_source: event.addr_or_source,
            value_or_len,
            dma_direction: event.dma_direction,
            pi_device: event.pi_device,
            pi_offset: event.pi_offset,
        }
    }
}

/// A compact, printable description of one event, for divergence reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSummary {
    pub ordinal: u64,
    pub event_kind: TimingEventKind,
    pub device: TimingDevice,
    pub cycle: u64,
    pub addr_or_source: u32,
    pub value_or_len: u32,
    pub dma_direction: Option<TimingDmaDirection>,
    pub pi_device: Option<TimingPiDevice>,
    pub pi_offset: Option<u32>,
}

impl EventSummary {
    fn of(event: &DeviceEvent) -> Self {
        Self {
            ordinal: event.ordinal,
            event_kind: event.event_kind,
            device: event.device,
            cycle: event.cycle,
            addr_or_source: event.addr_or_source,
            value_or_len: event.value_or_len,
            dma_direction: event.dma_direction,
            pi_device: event.pi_device,
            pi_offset: event.pi_offset,
        }
    }
}

impl std::fmt::Display for EventSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ordinal={} {:?}/{:?} cycle={} addr_or_source=0x{:x} value_or_len={} dma_direction={:?} pi_device={:?} pi_offset={:?}",
            self.ordinal,
            self.event_kind,
            self.device,
            self.cycle,
            self.addr_or_source,
            self.value_or_len,
            self.dma_direction,
            self.pi_device,
            self.pi_offset,
        )
    }
}

/// The first (and only reported) divergence between the two streams. The
/// comparator halts alignment at the first structural mismatch — per the spec's
/// "first divergent guest cycle" principle, the first divergence is the
/// actionable one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// The producers observed different device sets. A missing device cannot
    /// be interpreted as an empty, agreeing stream.
    Scope {
        fn64: Vec<TimingDevice>,
        reference: Vec<TimingDevice>,
    },
    /// Event bodies agreed within tolerance, but one or both trace envelopes
    /// did not report successful completion. Agreement requires both sides to
    /// be `Completed`; two matching `Aborted` streams are still failed evidence.
    Completion {
        /// Position immediately after the compared event prefix.
        index: usize,
        fn64: DeviceTraceCompletion,
        reference: DeviceTraceCompletion,
    },
    /// Zero-tolerance ordering mismatch: the aligned position `index` (0-based
    /// over the compared event sequence) does not agree on
    /// `(event_kind, device, payload)`. One side may be absent (a missing or
    /// extra event — one stream ran past the other).
    Ordering {
        /// 0-based index into the aligned event sequence.
        index: usize,
        /// What fn64 had at this position, if any.
        fn64: Option<EventSummary>,
        /// What the reference had at this position, if any.
        reference: Option<EventSummary>,
    },
    /// The aligned events agree on `(event_kind, device, payload)` but their
    /// cycle stamps differ by more than the device's tolerance band.
    CycleOutOfBand {
        /// 0-based index into the aligned event sequence.
        index: usize,
        fn64: EventSummary,
        reference: EventSummary,
        /// Absolute cycle difference `|fn64.cycle - reference.cycle|`.
        cycle_delta: u64,
        /// The device band the delta exceeded.
        tolerance: u64,
    },
}

impl Divergence {
    /// The aligned index this divergence occurred at.
    pub fn index(&self) -> usize {
        match self {
            Self::Scope { .. } => 0,
            Self::Completion { index, .. }
            | Self::Ordering { index, .. }
            | Self::CycleOutOfBand { index, .. } => *index,
        }
    }
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn side(label: &str, event: &Option<EventSummary>) -> String {
            match event {
                Some(event) => format!("{label}: {event}"),
                None => format!("{label}: <absent>"),
            }
        }
        match self {
            Self::Scope { fn64, reference } => write!(
                f,
                "observation-scope-mismatch (fn64={fn64:?}, reference={reference:?})",
            ),
            Self::Completion {
                index,
                fn64,
                reference,
            } => write!(
                f,
                "trace-completion-invalid after {index} aligned events: both traces must be Completed (fn64={fn64:?}, reference={reference:?})",
            ),
            Self::Ordering {
                index,
                fn64,
                reference,
            } => write!(
                f,
                "ordering-mismatch at aligned index {index}\n  {}\n  {}",
                side("fn64", fn64),
                side("reference", reference),
            ),
            Self::CycleOutOfBand {
                index,
                fn64,
                reference,
                cycle_delta,
                tolerance,
            } => write!(
                f,
                "cycle-out-of-band at aligned index {index} (delta={cycle_delta} > tolerance={tolerance})\n  fn64: {fn64}\n  reference: {reference}",
            ),
        }
    }
}

/// Summary counts over the aligned comparison, independent of whether a
/// divergence was found (the counts reflect what was actually compared up to
/// and including the first divergence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffCounts {
    /// Aligned positions compared (both sides present and structurally
    /// compatible). A divergence position is NOT counted here.
    pub events_compared: usize,
    /// Positions whose `(event_kind, device, payload)` matched. Equal to
    /// `events_compared` when no ordering divergence was hit.
    pub ordering_matches: usize,
    /// Aligned positions whose cycle stamps were within the device band.
    pub cycle_in_band: usize,
    /// Aligned positions whose cycle stamps were out of band. At most 1,
    /// because the comparator halts at the first divergence.
    pub cycle_out_of_band: usize,
    /// Total events on the fn64 side.
    pub fn64_events: usize,
    /// Total events on the reference side.
    pub reference_events: usize,
}

/// The structured diff report: the first divergence (if any), summary counts,
/// and the tolerance used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffReport {
    /// `None` means both streams completed and their events agree end-to-end
    /// within tolerance.
    pub first_divergence: Option<Divergence>,
    pub counts: DiffCounts,
    pub tolerance: TimingTolerance,
}

impl DiffReport {
    /// True only when both traces completed, ordering matches, and every cycle
    /// is in band.
    pub fn agrees(&self) -> bool {
        self.first_divergence.is_none()
    }

    /// A human-readable report matching the gate bins' style.
    pub fn to_human(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        writeln!(out, "=== fn64-discover timing diff ===").unwrap();
        writeln!(
            out,
            "tolerance (R4300 master cycles): pi={} si={} ai={} mi={} vi={}",
            self.tolerance.pi_cycles,
            self.tolerance.si_cycles,
            self.tolerance.ai_cycles,
            self.tolerance.mi_cycles,
            self.tolerance.vi_cycles,
        )
        .unwrap();
        writeln!(
            out,
            "fn64 events={} reference events={}",
            self.counts.fn64_events, self.counts.reference_events
        )
        .unwrap();
        writeln!(
            out,
            "compared={} ordering_matches={} cycle_in_band={} cycle_out_of_band={}",
            self.counts.events_compared,
            self.counts.ordering_matches,
            self.counts.cycle_in_band,
            self.counts.cycle_out_of_band,
        )
        .unwrap();
        match &self.first_divergence {
            None => writeln!(out, "RESULT: AGREE (no divergence within tolerance)").unwrap(),
            Some(divergence) => {
                writeln!(out, "RESULT: DIVERGE").unwrap();
                writeln!(out, "first divergence: {divergence}").unwrap();
            }
        }
        out
    }
}

/// Diff two device-event bodies. This internal helper has no trace-envelope
/// completion information; only [`diff_ingests`] can produce an acceptance
/// report.
///
/// The two streams are aligned position-for-position (index 0 to index 0, and
/// so on). At each position:
/// - if one side is exhausted and the other still has an event, that is an
///   ordering divergence (missing/extra event) — halt;
/// - if the two events' [`AlignmentKey`]s differ, that is an ordering
///   divergence (reordered/wrong event) — halt;
/// - otherwise the keys match; compare cycles against the device band. If out
///   of band, that is a cycle divergence — halt. If in band, advance.
///
/// The comparator halts at the FIRST divergence (spec's first-divergent
/// principle) and never resynchronizes past a structural mismatch, because any
/// pairing past an unmatched event would be a guess (`wrong == 0`).
fn diff_events(
    fn64: &[DeviceEvent],
    reference: &[DeviceEvent],
    tolerance: &TimingTolerance,
) -> DiffReport {
    let mut counts = DiffCounts {
        fn64_events: fn64.len(),
        reference_events: reference.len(),
        ..DiffCounts::default()
    };

    let mut index = 0usize;
    loop {
        let left = fn64.get(index);
        let right = reference.get(index);
        match (left, right) {
            (None, None) => {
                // Both exhausted together: agreement.
                return DiffReport {
                    first_divergence: None,
                    counts,
                    tolerance: *tolerance,
                };
            }
            (None, Some(_)) | (Some(_), None) => {
                // One side ran past the other — a missing or extra event.
                return DiffReport {
                    first_divergence: Some(Divergence::Ordering {
                        index,
                        fn64: left.map(EventSummary::of),
                        reference: right.map(EventSummary::of),
                    }),
                    counts,
                    tolerance: *tolerance,
                };
            }
            (Some(left), Some(right)) => {
                if AlignmentKey::of(left) != AlignmentKey::of(right) {
                    // Structural (ordering/payload) mismatch: zero tolerance.
                    return DiffReport {
                        first_divergence: Some(Divergence::Ordering {
                            index,
                            fn64: Some(EventSummary::of(left)),
                            reference: Some(EventSummary::of(right)),
                        }),
                        counts,
                        tolerance: *tolerance,
                    };
                }
                // Keys match — this position's ordering agrees.
                counts.ordering_matches += 1;
                let band = tolerance.band_for(left.device);
                let delta = left.cycle.abs_diff(right.cycle);
                if delta > band {
                    counts.cycle_out_of_band += 1;
                    return DiffReport {
                        first_divergence: Some(Divergence::CycleOutOfBand {
                            index,
                            fn64: EventSummary::of(left),
                            reference: EventSummary::of(right),
                            cycle_delta: delta,
                            tolerance: band,
                        }),
                        counts,
                        tolerance: *tolerance,
                    };
                }
                counts.cycle_in_band += 1;
                counts.events_compared += 1;
                index += 1;
            }
        }
    }
}

/// Diff two fully-ingested traces. Event diagnostics take precedence: the first
/// ordering or cycle divergence is preserved even when a trace also aborted.
/// If the event bodies agree, both envelopes must be `Completed`; any aborted
/// side becomes a [`Divergence::Completion`].
pub fn diff_ingests(
    fn64: &DeviceTraceIngest,
    reference: &DeviceTraceIngest,
    tolerance: &TimingTolerance,
) -> DiffReport {
    if fn64.header.observed_devices != reference.header.observed_devices {
        return DiffReport {
            first_divergence: Some(Divergence::Scope {
                fn64: fn64.header.observed_devices.clone(),
                reference: reference.header.observed_devices.clone(),
            }),
            counts: DiffCounts {
                fn64_events: fn64.events.len(),
                reference_events: reference.events.len(),
                ..DiffCounts::default()
            },
            tolerance: *tolerance,
        };
    }
    let mut report = diff_events(&fn64.events, &reference.events, tolerance);
    if report.first_divergence.is_none()
        && (fn64.completion != DeviceTraceCompletion::Completed
            || reference.completion != DeviceTraceCompletion::Completed)
    {
        report.first_divergence = Some(Divergence::Completion {
            index: report.counts.events_compared,
            fn64: fn64.completion,
            reference: reference.completion,
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing_trace::{
        DeviceTraceCompletion, DeviceTraceRecord, TimingDmaDirection, TimingPiDevice,
        TimingTraceClock, DEVICE_TRACE_SCHEMA_VERSION,
    };

    /// Build a small but representative device-event stream: a PI DMA
    /// start/complete, the MI raise it triggers, an SI DMA (fixed window,
    /// value_or_len=0), and a VI retrace. Returned as the record sequence so
    /// tests can round-trip through the schema and mutate it.
    fn sample_records() -> Vec<DeviceTraceRecord> {
        vec![
            DeviceTraceRecord::Header {
                ordinal: 0,
                schema_version: DEVICE_TRACE_SCHEMA_VERSION,
                clock: TimingTraceClock::exact_master_cycles(),
                observed_devices: vec![
                    TimingDevice::Pi,
                    TimingDevice::Ai,
                    TimingDevice::Si,
                    TimingDevice::Vi,
                    TimingDevice::Mi,
                ],
                producer: "synthetic".to_string(),
                trace_id: "diff-sample".to_string(),
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 1,
                event_kind: TimingEventKind::DmaStart,
                device: TimingDevice::Pi,
                cycle: 0,
                addr_or_source: 0x0020,
                value_or_len: 0x1000,
                dma_direction: Some(TimingDmaDirection::ToRdram),
                pi_device: Some(TimingPiDevice::Rom),
                pi_offset: Some(0x10),
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 2,
                event_kind: TimingEventKind::DmaComplete,
                device: TimingDevice::Pi,
                cycle: 131_142,
                addr_or_source: 0x0020,
                value_or_len: 0x1000,
                dma_direction: Some(TimingDmaDirection::ToRdram),
                pi_device: Some(TimingPiDevice::Rom),
                pi_offset: Some(0x10),
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 3,
                event_kind: TimingEventKind::MiRaise,
                device: TimingDevice::Mi,
                cycle: 131_142,
                addr_or_source: 1 << 4, // PI source bit
                value_or_len: 0,
                dma_direction: None,
                pi_device: None,
                pi_offset: None,
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 4,
                event_kind: TimingEventKind::DmaStart,
                device: TimingDevice::Si,
                cycle: 138_964,
                addr_or_source: 0x0040,
                value_or_len: 0, // fixed 64-byte PIF window: fn64 emits 0
                dma_direction: None,
                pi_device: None,
                pi_offset: None,
            },
            DeviceTraceRecord::DeviceEvent {
                ordinal: 5,
                event_kind: TimingEventKind::ViRetrace,
                device: TimingDevice::Vi,
                cycle: 238_964,
                addr_or_source: 0,
                value_or_len: 0,
                dma_direction: None,
                pi_device: None,
                pi_offset: None,
            },
            DeviceTraceRecord::End {
                ordinal: 6,
                completion: DeviceTraceCompletion::Completed,
            },
        ]
    }

    /// Ingest a record sequence back into the strongly-typed event stream,
    /// exercising the real schema envelope (header/ordinals/end) on the way.
    fn ingest(records: &[DeviceTraceRecord]) -> DeviceTraceIngest {
        use crate::timing_trace::{ingest_jsonl, to_jsonl};
        use std::io::Cursor;
        let jsonl = to_jsonl(records).expect("serialize sample");
        ingest_jsonl(Cursor::new(jsonl)).expect("ingest sample")
    }

    /// Renumber a record sequence's ordinals densely (0,1,2,...) so a mutated
    /// stream (event dropped/injected) still satisfies the gap-free envelope.
    fn renumber(mut records: Vec<DeviceTraceRecord>) -> Vec<DeviceTraceRecord> {
        for (index, record) in records.iter_mut().enumerate() {
            let ordinal = index as u64;
            match record {
                DeviceTraceRecord::Header { ordinal: o, .. }
                | DeviceTraceRecord::DeviceEvent { ordinal: o, .. }
                | DeviceTraceRecord::End { ordinal: o, .. } => *o = ordinal,
            }
        }
        records
    }

    // --- Self-test A: two identical streams agree, zero divergences. ---
    #[test]
    fn test_a_identical_streams_agree() {
        let records = sample_records();
        let fn64 = ingest(&records);
        let reference = ingest(&records);
        let report = diff_ingests(&fn64, &reference, &TimingTolerance::initial_loose());

        assert!(report.agrees(), "identical streams must agree: {report:?}");
        assert_eq!(report.first_divergence, None);
        assert_eq!(report.counts.events_compared, 5);
        assert_eq!(report.counts.ordering_matches, 5);
        assert_eq!(report.counts.cycle_in_band, 5);
        assert_eq!(report.counts.cycle_out_of_band, 0);
        assert_eq!(report.counts.fn64_events, 5);
        assert_eq!(report.counts.reference_events, 5);
    }

    #[test]
    fn equal_empty_aborted_traces_cannot_report_agreement() {
        let records = vec![
            DeviceTraceRecord::Header {
                ordinal: 0,
                schema_version: DEVICE_TRACE_SCHEMA_VERSION,
                clock: TimingTraceClock::exact_master_cycles(),
                observed_devices: vec![TimingDevice::Vi],
                producer: "synthetic".to_string(),
                trace_id: "empty-abort".to_string(),
            },
            DeviceTraceRecord::End {
                ordinal: 1,
                completion: DeviceTraceCompletion::Aborted,
            },
        ];
        let fn64 = ingest(&records);
        let reference = ingest(&records);
        let report = diff_ingests(&fn64, &reference, &TimingTolerance::initial_loose());

        assert!(!report.agrees());
        assert_eq!(report.counts.events_compared, 0);
        assert!(matches!(
            report.first_divergence,
            Some(Divergence::Completion {
                index: 0,
                fn64: DeviceTraceCompletion::Aborted,
                reference: DeviceTraceCompletion::Aborted,
            })
        ));
        let diagnostic = report.to_human();
        assert!(diagnostic.contains("RESULT: DIVERGE"));
        assert!(diagnostic.contains("fn64=Aborted, reference=Aborted"));
    }

    #[test]
    fn mismatched_observation_scopes_cannot_report_agreement() {
        let fn64 = ingest(&sample_records());
        let mut reference = ingest(&sample_records());
        reference.header.observed_devices = vec![TimingDevice::Vi];
        let report = diff_ingests(&fn64, &reference, &TimingTolerance::initial_loose());
        assert!(matches!(
            report.first_divergence,
            Some(Divergence::Scope { .. })
        ));
        assert_eq!(report.counts.events_compared, 0);
        assert!(report.to_human().contains("observation-scope-mismatch"));
    }

    #[test]
    fn completion_mismatch_fails_after_preserving_matched_event_counts() {
        let completed = ingest(&sample_records());
        let mut aborted_records = sample_records();
        let Some(DeviceTraceRecord::End { completion, .. }) = aborted_records.last_mut() else {
            panic!("sample ends with completion");
        };
        *completion = DeviceTraceCompletion::Aborted;
        let aborted = ingest(&aborted_records);

        let report = diff_ingests(&completed, &aborted, &TimingTolerance::initial_loose());
        assert!(!report.agrees());
        assert_eq!(report.counts.events_compared, 5);
        assert_eq!(report.counts.ordering_matches, 5);
        assert_eq!(report.counts.cycle_in_band, 5);
        assert!(matches!(
            report.first_divergence,
            Some(Divergence::Completion {
                index: 5,
                fn64: DeviceTraceCompletion::Completed,
                reference: DeviceTraceCompletion::Aborted,
            })
        ));
    }

    #[test]
    fn event_divergence_remains_the_first_diagnostic_when_a_trace_aborted() {
        let reference = ingest(&sample_records());
        let mut aborted_records = sample_records();
        for record in &mut aborted_records {
            match record {
                DeviceTraceRecord::DeviceEvent {
                    event_kind: TimingEventKind::DmaStart,
                    device: TimingDevice::Pi,
                    value_or_len,
                    ..
                } => *value_or_len += 1,
                DeviceTraceRecord::End { completion, .. } => {
                    *completion = DeviceTraceCompletion::Aborted;
                }
                _ => {}
            }
        }
        let aborted = ingest(&aborted_records);
        let report = diff_ingests(&aborted, &reference, &TimingTolerance::initial_loose());

        assert!(matches!(
            report.first_divergence,
            Some(Divergence::Ordering { index: 0, .. })
        ));
    }

    #[test]
    fn equal_pi_offsets_on_different_devices_are_an_ordering_divergence() {
        let rom = ingest(&sample_records());
        let mut sram_records = sample_records();
        for record in &mut sram_records {
            if let DeviceTraceRecord::DeviceEvent {
                device: TimingDevice::Pi,
                pi_device,
                ..
            } = record
            {
                *pi_device = Some(TimingPiDevice::Sram);
            }
        }
        let sram = ingest(&sram_records);
        let report = diff_ingests(&rom, &sram, &TimingTolerance::initial_loose());
        assert!(matches!(
            report.first_divergence,
            Some(Divergence::Ordering { index: 0, .. })
        ));
    }

    // --- Self-test B: inject a wrong PI completion cycle beyond the band; the
    // comparator flags it at the right ordinal as cycle-out-of-band. This
    // proves the oracle detects a REAL timing bug, not just agreement. ---
    #[test]
    fn test_b_injected_wrong_pi_cycle_is_flagged_out_of_band() {
        let reference_records = sample_records();
        let reference = ingest(&reference_records);

        // fn64's PI completion is pushed far past the reference's, beyond the
        // PI band (4096) — simulating a wrong PI latency model (exactly the U2
        // bug this oracle is meant to catch).
        let mut fn64 = ingest(&sample_records());
        let bad_cycle = 131_142 + 50_000;
        for event in &mut fn64.events {
            if let DeviceEvent {
                event_kind: TimingEventKind::DmaComplete,
                device: TimingDevice::Pi,
                cycle,
                ..
            } = event
            {
                *cycle = bad_cycle;
            }
        }

        let tolerance = TimingTolerance::initial_loose();
        let report = diff_ingests(&fn64, &reference, &tolerance);

        assert!(!report.agrees(), "an out-of-band PI cycle must diverge");
        match report.first_divergence {
            Some(Divergence::CycleOutOfBand {
                index,
                fn64: fn64_event,
                reference: reference_event,
                cycle_delta,
                tolerance: band,
            }) => {
                // The PI complete is aligned index 1 (index 0 is the PI start).
                assert_eq!(index, 1, "must flag the PI-complete position");
                // ...which is ordinal 2 in the original stream.
                assert_eq!(fn64_event.ordinal, 2);
                assert_eq!(fn64_event.event_kind, TimingEventKind::DmaComplete);
                assert_eq!(fn64_event.device, TimingDevice::Pi);
                assert_eq!(fn64_event.cycle, bad_cycle);
                assert_eq!(reference_event.cycle, 131_142);
                assert_eq!(cycle_delta, 50_000);
                assert_eq!(band, tolerance.pi_cycles);
            }
            other => panic!("expected CycleOutOfBand, got {other:?}"),
        }
        // The PI start (index 0) was compared and in-band before the divergence.
        assert_eq!(report.counts.events_compared, 1);
        assert_eq!(report.counts.ordering_matches, 2);
        assert_eq!(report.counts.cycle_in_band, 1);
        assert_eq!(report.counts.cycle_out_of_band, 1);
    }

    // A cycle difference AT the band edge is in-band; one cycle beyond is not.
    #[test]
    fn test_b_band_boundary_is_inclusive() {
        let reference = ingest(&sample_records());

        let make_fn64 = |pi_complete: u64| {
            let mut ingest = ingest(&sample_records());
            for event in &mut ingest.events {
                if let DeviceEvent {
                    event_kind: TimingEventKind::DmaComplete,
                    device: TimingDevice::Pi,
                    cycle,
                    ..
                } = event
                {
                    *cycle = pi_complete;
                }
            }
            ingest
        };
        let tolerance = TimingTolerance::initial_loose();

        // Exactly at the band edge: in-band, agrees.
        let at_edge = make_fn64(131_142 + tolerance.pi_cycles);
        assert!(diff_ingests(&at_edge, &reference, &tolerance).agrees());

        // One cycle past the edge: out of band, diverges.
        let past_edge = make_fn64(131_142 + tolerance.pi_cycles + 1);
        let report = diff_ingests(&past_edge, &reference, &tolerance);
        assert!(matches!(
            report.first_divergence,
            Some(Divergence::CycleOutOfBand { .. })
        ));
    }

    // --- Self-test C: drop one event; the comparator flags it as an ordering
    // mismatch at that position (missing event). ---
    #[test]
    fn test_c_dropped_event_is_ordering_mismatch() {
        let reference = ingest(&sample_records());

        // fn64 is missing the MI raise (original ordinal 3, aligned index 2):
        // after the PI complete, fn64 jumps straight to the SI start while the
        // reference still has the MI raise there.
        let mut fn64_records = sample_records();
        fn64_records.retain(|record| {
            !matches!(
                record,
                DeviceTraceRecord::DeviceEvent {
                    event_kind: TimingEventKind::MiRaise,
                    ..
                }
            )
        });
        let fn64 = ingest(&renumber(fn64_records));

        let report = diff_ingests(&fn64, &reference, &TimingTolerance::initial_loose());
        assert!(!report.agrees(), "a dropped event must diverge");
        match report.first_divergence {
            Some(Divergence::Ordering {
                index,
                fn64: fn64_event,
                reference: reference_event,
            }) => {
                // Aligned index 2: PI start (0), PI complete (1) matched; the
                // MI raise is where fn64 diverges.
                assert_eq!(index, 2);
                // fn64 has the SI start here; the reference has the MI raise.
                let fn64_event = fn64_event.expect("fn64 still has an event here");
                assert_eq!(fn64_event.device, TimingDevice::Si);
                assert_eq!(fn64_event.event_kind, TimingEventKind::DmaStart);
                let reference_event = reference_event.expect("reference has the MI raise");
                assert_eq!(reference_event.device, TimingDevice::Mi);
                assert_eq!(reference_event.event_kind, TimingEventKind::MiRaise);
            }
            other => panic!("expected Ordering, got {other:?}"),
        }
        // The two PI events matched before the divergence.
        assert_eq!(report.counts.ordering_matches, 2);
    }

    // A stream that is a strict prefix of the other diverges at the first
    // unpaired event: the shorter side is "missing" it (absent), never
    // silently accepted as agreement.
    #[test]
    fn test_c_prefix_stream_flags_missing_tail() {
        let reference = ingest(&sample_records());

        // fn64 stops after the PI start (drop everything after ordinal 1).
        let mut fn64_records = sample_records();
        fn64_records.retain(|record| {
            !matches!(
                record,
                DeviceTraceRecord::DeviceEvent { ordinal, .. } if *ordinal >= 2
            )
        });
        let fn64 = ingest(&renumber(fn64_records));

        let report = diff_ingests(&fn64, &reference, &TimingTolerance::initial_loose());
        match report.first_divergence {
            Some(Divergence::Ordering {
                index,
                fn64: fn64_event,
                reference: reference_event,
            }) => {
                assert_eq!(index, 1);
                assert!(fn64_event.is_none(), "fn64 is exhausted");
                let reference_event = reference_event.expect("reference has more events");
                assert_eq!(reference_event.event_kind, TimingEventKind::DmaComplete);
            }
            other => panic!("expected Ordering (missing tail), got {other:?}"),
        }
    }

    // --- Self-test D: an SI value_or_len difference is NOT flagged (the
    // fixed-window caveat). fn64 emits 0; a reference may emit the true 64. ---
    #[test]
    fn test_d_si_value_or_len_difference_is_not_flagged() {
        // fn64 keeps SI value_or_len = 0 (its fabric's fixed-window emission).
        let fn64 = ingest(&sample_records());

        // The reference emits the true 64-byte SI length AND a different length
        // for the same SI start — neither must be flagged.
        let mut reference_records = sample_records();
        for record in &mut reference_records {
            if let DeviceTraceRecord::DeviceEvent {
                device: TimingDevice::Si,
                value_or_len,
                ..
            } = record
            {
                *value_or_len = 64;
            }
        }
        let reference = ingest(&reference_records);

        let report = diff_ingests(&fn64, &reference, &TimingTolerance::initial_loose());
        assert!(
            report.agrees(),
            "SI value_or_len differences must not diverge (fixed-window caveat): {report:?}"
        );
        assert_eq!(report.counts.events_compared, 5);

        // Sanity: for a NON-SI device the same payload difference DOES diverge,
        // proving the exemption is specific to SI, not a blanket ignore.
        let mut pi_len_records = sample_records();
        for record in &mut pi_len_records {
            if let DeviceTraceRecord::DeviceEvent {
                device: TimingDevice::Pi,
                event_kind: TimingEventKind::DmaStart,
                value_or_len,
                ..
            } = record
            {
                *value_or_len = 0x2000; // reference disagrees on PI length
            }
        }
        let pi_len_reference = ingest(&pi_len_records);
        let pi_report = diff_ingests(&fn64, &pi_len_reference, &TimingTolerance::initial_loose());
        assert!(
            matches!(
                pi_report.first_divergence,
                Some(Divergence::Ordering { .. })
            ),
            "a PI length mismatch is a payload/ordering divergence: {pi_report:?}"
        );
        assert_eq!(pi_report.first_divergence.as_ref().unwrap().index(), 0);
    }

    // The comparator itself is deterministic: same inputs -> identical report.
    #[test]
    fn diff_is_deterministic() {
        let fn64 = ingest(&sample_records());
        let reference = ingest(&sample_records());
        let tolerance = TimingTolerance::initial_loose();
        let first = diff_ingests(&fn64, &reference, &tolerance);
        let second = diff_ingests(&fn64, &reference, &tolerance);
        assert_eq!(first, second);
    }

    // The default tolerance is the documented initial-loose band.
    #[test]
    fn default_tolerance_is_initial_loose() {
        assert_eq!(TimingTolerance::default(), TimingTolerance::initial_loose());
        let tolerance = TimingTolerance::initial_loose();
        assert_eq!(tolerance.band_for(TimingDevice::Pi), 4096);
        assert_eq!(tolerance.band_for(TimingDevice::Si), 2048);
        assert_eq!(tolerance.band_for(TimingDevice::Ai), 2048);
        assert_eq!(tolerance.band_for(TimingDevice::Mi), 512);
        assert_eq!(tolerance.band_for(TimingDevice::Vi), 256);
    }
}
