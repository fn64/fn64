//! **What this build is running on** -- the shell's stack identity, printed
//! unconditionally at startup and at exit, and available on screen (F3).
//!
//! ## Why this module exists
//!
//! A shell run's stack is chosen in two places that a player never sees:
//!
//! - the **recompiler lane** is fixed at BUILD time (`build.rs` emits
//!   `cfg(fn64_cpu_runtime)` only for `FN64_RECOMP=rs`; otherwise the game
//!   comes from N64Recomp's generated C compiled through the bridge), and
//! - the **renderer** is chosen at boot from `FN64_RENDER`, which defaults to
//!   the software `ReferenceBackend` and may silently fall back to it when a
//!   requested backend fails to construct.
//!
//! Both defaults are the opposite of the intended target stack
//! (`fn64-cpu-runtime` + `fn64-render-wgpu`), and neither was fully reported.
//! `main.rs` did print the renderer; **nothing printed the lane at all**, so a
//! session running the C lane against the reference rasterizer looked, from
//! its own log, exactly like one running the Rust lane against wgpu. Symptom
//! reports arrived without their cell and had to have it inferred after the
//! fact -- twice in one day, wrongly both times.
//!
//! ## The rule this module enforces
//!
//! Every field here is derived from the thing that DECIDES it, never from a
//! restatement of it:
//!
//! - `RECOMPILER_LANE` is `#[cfg]`-selected, so it cannot drift from the cfg
//!   `build.rs` set. A runtime read of `FN64_RECOMP` would report the
//!   *request* made to the last build, which is not the same value: the
//!   variable can be changed, unset, or differ from the one the linked binary
//!   was compiled with, and the binary would keep reporting the new value
//!   while running the old lane. `stack_line_tracks_the_compiled_lane` in the
//!   tests below is the mutation guard for exactly that.
//! - the renderer string is the `active_renderer` `boot()` produced when it
//!   registered the backend, threaded through -- not a re-read of
//!   `FN64_RENDER`, which reports the REQUEST and would print `wgpu` for a run
//!   that fell back to the reference oracle.

/// Which recompiler produced the linked game bodies.
///
/// `#[cfg]`-selected from the cfg `build.rs` sets (`fn64_cpu_runtime` for
/// `FN64_RECOMP=rs`). Not a runtime inference -- see the module doc.
#[cfg(fn64_cpu_runtime)]
pub const RECOMPILER_LANE: &str = "rs (fn64-cpu-runtime)";
#[cfg(not(fn64_cpu_runtime))]
pub const RECOMPILER_LANE: &str = "c (N64Recomp C bodies via bridge)";

/// Whether a game is linked into this binary at all.
///
/// Same cfg the `game` module in `main.rs` is gated on, so this cannot claim a
/// linked game in a binary that has none.
#[cfg(fn64_game_linked)]
pub const GAME_LINK: &str = "linked";
#[cfg(not(fn64_game_linked))]
pub const GAME_LINK: &str = "none (content-free build)";

/// The build-time provenance of the linked bodies: the `RECOMPILED_DIR` (C
/// lane) or `RECOMP_RS_HOST_LOOKUP` (rs lane) the build actually consumed.
/// `build.rs` emits it; absent in a content-free build.
pub const GAME_SOURCE: Option<&str> = option_env!("FN64_SHELL_GAME_SOURCE");

/// The ROM path the build validated. Runtime `ROM` may differ (the shell reads
/// it fresh at boot), which is itself worth being able to see.
pub const BUILD_ROM: Option<&str> = option_env!("FN64_SHELL_BUILD_ROM");

/// One greppable block naming everything that decides what a run does.
///
/// Printed unconditionally at startup AND at exit: a banner scrolled off the
/// top of a 20-minute session's log helps nobody, and reprinting it costs one
/// `println!`. Every line carries the `[fn64-stack]` prefix so a user can
/// `grep fn64-stack` a log and paste the result into a report.
pub fn banner(active_renderer: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("[fn64-stack] ---- this build is running on ----\n");
    out.push_str(&format!("[fn64-stack] recompiler : {RECOMPILER_LANE}\n"));
    match active_renderer {
        Some(renderer) => {
            out.push_str(&format!(
                "[fn64-stack] renderer   : {}{}\n",
                renderer,
                renderer_caveat(renderer)
            ));
        }
        // The startup banner prints BEFORE the ROM loads and before a backend
        // is constructed, so it cannot name a renderer yet without lying about
        // one. Name the request and say plainly that it is a request.
        None => {
            let requested = requested_renderer();
            out.push_str(&format!(
                "[fn64-stack] renderer   : {requested} (REQUESTED via FN64_RENDER; the \
                 registered backend is reported when it is created)\n"
            ));
        }
    }
    out.push_str(&format!("[fn64-stack] game       : {GAME_LINK}\n"));
    if let Some(source) = GAME_SOURCE {
        out.push_str(&format!("[fn64-stack] built from : {source}\n"));
    }
    if let Some(rom) = BUILD_ROM {
        out.push_str(&format!("[fn64-stack] build ROM  : {rom}\n"));
    }
    out.push_str("[fn64-stack] -------------------------------");
    out
}

/// What `FN64_RENDER` asks for, normalized the same way `boot()` normalizes
/// it. A REQUEST, never an outcome: `boot()` may fall back.
pub fn requested_renderer() -> String {
    std::env::var("FN64_RENDER")
        .unwrap_or_else(|_| "reference".to_string())
        .to_ascii_lowercase()
}

/// Why a renderer name deserves a second look, appended to the banner line.
///
/// `reference-fallback` is the one that matters: it means a backend was
/// REQUESTED and failed, and the run silently continued on the software
/// oracle. Without this the string is one hyphenated word away from
/// `reference` in a log a human is skimming.
pub fn renderer_caveat(active_renderer: &str) -> &'static str {
    match active_renderer {
        "reference-fallback" => {
            " <== FELL BACK: the requested backend failed to create; see the WARNING above"
        }
        "reference" => " (software oracle -- the default when FN64_RENDER is unset)",
        _ => "",
    }
}

/// The heads-up display's fixed identity rows: what this build runs on,
/// independent of the frame it is on. Framerate is appended live by the
/// caller (see `HudSample`).
pub fn hud_identity(active_renderer: &str) -> [(&'static str, String); 2] {
    [
        ("CPU", RECOMPILER_LANE.to_string()),
        (
            "GPU",
            format!("{active_renderer}{}", hud_renderer_flag(active_renderer)),
        ),
    ]
}

/// The HUD has no room for the banner's full sentence, so the fallback state
/// gets a short marker instead. Silence for every other backend.
fn hud_renderer_flag(active_renderer: &str) -> &'static str {
    if active_renderer == "reference-fallback" {
        "  ! FELL BACK"
    } else {
        ""
    }
}

/// The live half of the HUD.
///
/// ## What is shown, and what is deliberately NOT
///
/// A threshold-crossing percentage over frame INTERVALS is the shape this
/// deliberately avoids. One was added earlier and reported "57.3% of pumps
/// over budget"; it counted intervals against 16,666,667 ns while the interval
/// median sat exactly on that value, so it was a coin flip on microsecond
/// scheduler jitter. The real over-budget rate by pump COST was 0.1%
/// (9 of 6000). A statistic whose value is decided by jitter around its own
/// threshold cannot distinguish two lanes whose costs differ threefold, and it
/// sent a whole investigation after a tail that did not exist.
///
/// So the HUD shows a DISTRIBUTION, and shows it over the quantity that
/// characterizes work:
///
/// - `fps` -- the short-window presented rate, from the frame interval median.
///   An interval is the right quantity for "how fast is it going"; it is only
///   the wrong one for "how much work is it doing".
/// - `pump_*` -- p50/p95/max of PUMP COST, labeled as such. The pump is the
///   guest+graphics work inside one field, and it is what differs between
///   stacks (measured the same day: 4.4 ms on rt64 vs 14.6 ms on reference
///   against a 16.67 ms budget). p50 next to p95 next to max says whether a
///   run is uniformly slow or jittery without collapsing either into a
///   percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudSample {
    pub fps: f64,
    pub pump_p50_ms: f64,
    pub pump_p95_ms: f64,
    pub pump_max_ms: f64,
    pub samples: usize,
}

impl HudSample {
    /// The single live line under the identity rows. Every number names its
    /// quantity: `pump` is pump COST, never the frame interval, because the
    /// two differ by the wait and confusing them is the documented artifact.
    pub fn line(&self) -> String {
        format!(
            "{:.1} fps · pump p50/p95/max {:.1}/{:.1}/{:.1} ms · n={}",
            self.fps, self.pump_p50_ms, self.pump_p95_ms, self.pump_max_ms, self.samples
        )
    }
}

/// A small, self-contained rolling window feeding the HUD.
///
/// Deliberately separate from the heartbeat's `TimingWindow`s: those are
/// DRAINED by `take_stats()` every 60 swaps, so reading them for the HUD would
/// either steal the heartbeat's samples or force the HUD to a 1-second
/// refresh. This keeps its own bounded ring and refreshes on its own cadence,
/// so the two instruments never take each other's data.
///
/// Cost per pump is two pushes into a fixed-capacity ring and two index
/// bumps -- no allocation, no clock the pump did not already read.
pub struct HudTiming {
    intervals_ms: [f64; HUD_WINDOW],
    pumps_ms: [f64; HUD_WINDOW],
    next: usize,
    filled: usize,
    last_refresh: Option<std::time::Instant>,
    current: Option<HudSample>,
}

/// One second at 60 Hz. Long enough that a single scheduler hiccup does not
/// dominate the readout, short enough that the HUD tracks what is happening
/// now rather than averaging across a scene change.
const HUD_WINDOW: usize = 60;

/// How often the displayed numbers are recomputed. A HUD that re-sorts its
/// window every frame is itself a cost on the frame it measures; four updates
/// a second is faster than a reader can follow anyway.
const HUD_REFRESH: std::time::Duration = std::time::Duration::from_millis(250);

impl Default for HudTiming {
    fn default() -> Self {
        Self {
            intervals_ms: [0.0; HUD_WINDOW],
            pumps_ms: [0.0; HUD_WINDOW],
            next: 0,
            filled: 0,
            last_refresh: None,
            current: None,
        }
    }
}

impl HudTiming {
    /// Record one pump: the wall interval since the previous pump, and the
    /// cost of the pump itself. Both are already measured by the caller for
    /// the heartbeat, so this adds no timer to the hot loop.
    pub fn record(&mut self, interval: std::time::Duration, pump_cost: std::time::Duration) {
        self.intervals_ms[self.next] = interval.as_secs_f64() * 1000.0;
        self.pumps_ms[self.next] = pump_cost.as_secs_f64() * 1000.0;
        self.next = (self.next + 1) % HUD_WINDOW;
        self.filled = (self.filled + 1).min(HUD_WINDOW);
    }

    /// The sample to paint, recomputed at most every `HUD_REFRESH`.
    ///
    /// `None` until the window holds enough samples to mean anything. A HUD
    /// that shows a number from three frames is not cheaper than one that
    /// says it is still measuring, and it is less honest.
    pub fn sample(&mut self, now: std::time::Instant) -> Option<HudSample> {
        const MINIMUM: usize = 10;
        if self.filled < MINIMUM {
            return None;
        }
        let due = self
            .last_refresh
            .is_none_or(|last| now.duration_since(last) >= HUD_REFRESH);
        if due {
            self.last_refresh = Some(now);
            self.current = Some(self.compute());
        }
        self.current
    }

    fn compute(&self) -> HudSample {
        let mut intervals: Vec<f64> = self.intervals_ms[..self.filled].to_vec();
        let mut pumps: Vec<f64> = self.pumps_ms[..self.filled].to_vec();
        intervals.sort_by(f64::total_cmp);
        pumps.sort_by(f64::total_cmp);
        // fps from the interval MEDIAN, not the mean: one 200 ms stall while a
        // shader compiles would drag a mean for the rest of the window, and
        // the max below already reports that stall without hiding the rate.
        let interval_p50 = nearest_rank(&intervals, 50);
        HudSample {
            fps: if interval_p50 > 0.0 {
                1000.0 / interval_p50
            } else {
                0.0
            },
            pump_p50_ms: nearest_rank(&pumps, 50),
            pump_p95_ms: nearest_rank(&pumps, 95),
            pump_max_ms: *pumps.last().expect("filled >= MINIMUM > 0"),
            samples: self.filled,
        }
    }
}

/// Nearest-rank, matching `timing.rs`'s definition so the HUD and the
/// heartbeat cannot disagree about what "p95" means.
fn nearest_rank(sorted: &[f64], percentile: usize) -> f64 {
    let rank = (percentile * sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// **Mutation guard for the label this whole module exists to make
    /// trustworthy.** A lane string that cannot be wrong is worthless; this
    /// one exists precisely to be trusted, so it must track the cfg rather
    /// than be a literal that happens to match today's build.
    ///
    /// The test asserts the constant against the cfg through a SECOND,
    /// independently written derivation of the same condition. Replace
    /// `RECOMPILER_LANE`'s definition with either literal unconditionally and
    /// exactly one of the two cfg arms of this test fails.
    #[test]
    fn recompiler_lane_tracks_the_compiled_cfg_not_a_literal() {
        let compiled_lane_is_rs = cfg!(fn64_cpu_runtime);
        let reported_lane_is_rs = RECOMPILER_LANE.starts_with("rs");
        assert_eq!(
            reported_lane_is_rs, compiled_lane_is_rs,
            "RECOMPILER_LANE={RECOMPILER_LANE:?} disagrees with cfg(fn64_cpu_runtime)={compiled_lane_is_rs}: \
             the reported lane must be selected BY the cfg, never a literal that matches it today"
        );
        // And the two lanes must not be spelled the same, or the assertion
        // above would hold for a constant that says nothing.
        let other = if compiled_lane_is_rs {
            "c (N64Recomp C bodies via bridge)"
        } else {
            "rs (fn64-cpu-runtime)"
        };
        assert_ne!(RECOMPILER_LANE, other);
    }

    #[test]
    fn game_link_tracks_the_compiled_cfg() {
        assert_eq!(
            GAME_LINK == "linked",
            cfg!(fn64_game_linked),
            "GAME_LINK={GAME_LINK:?} must be selected by cfg(fn64_game_linked)"
        );
    }

    /// The banner is the paste-into-a-bug-report artifact, so its shape is
    /// part of the contract: one greppable prefix on every line, and the two
    /// facts that were invisible.
    #[test]
    fn banner_names_the_lane_and_the_registered_renderer() {
        let text = banner(Some("wgpu"));
        for line in text.lines() {
            assert!(
                line.starts_with("[fn64-stack]"),
                "every banner line must be greppable by one prefix, got {line:?}"
            );
        }
        assert!(text.contains(RECOMPILER_LANE), "banner must name the lane");
        assert!(
            text.contains("renderer   : wgpu"),
            "banner must name the REGISTERED renderer, got:\n{text}"
        );
        assert!(text.contains(GAME_LINK));
    }

    /// **Mutation guard for the silent-fallback case.** A backend that was
    /// requested, failed, and was replaced by the software oracle must not
    /// read like an ordinary `reference` run.
    #[test]
    fn a_fallback_renderer_is_visibly_distinct_from_a_requested_reference() {
        let fell_back = banner(Some("reference-fallback"));
        let chose_reference = banner(Some("reference"));

        assert!(
            fell_back.contains("FELL BACK"),
            "a silent fallback must be loud in the banner, got:\n{fell_back}"
        );
        assert!(
            !chose_reference.contains("FELL BACK"),
            "an ordinary reference run must not claim a fallback, got:\n{chose_reference}"
        );
        // Not merely different text -- the fallback marker is what a skimming
        // reader sees, and it must survive the HUD's tighter budget too.
        assert!(hud_identity("reference-fallback")[1]
            .1
            .contains("FELL BACK"));
        assert!(!hud_identity("reference")[1].1.contains("FELL BACK"));
    }

    /// Before a backend exists there is nothing to report but the request, and
    /// the banner must say which of the two it is printing.
    #[test]
    fn a_pre_boot_banner_labels_the_renderer_as_a_request() {
        let text = banner(None);
        assert!(
            text.contains("REQUESTED via FN64_RENDER"),
            "a pre-boot banner must not present a request as an outcome, got:\n{text}"
        );
    }

    #[test]
    fn hud_identity_reports_the_active_renderer_verbatim() {
        let rows = hud_identity("rt64");
        assert_eq!(rows[0].0, "CPU");
        assert_eq!(rows[0].1, RECOMPILER_LANE);
        assert_eq!(rows[1].0, "GPU");
        assert_eq!(
            rows[1].1, "rt64",
            "the HUD shows the backend that was registered, verbatim"
        );
    }

    /// The live line must label pump cost AS pump cost. The artifact it
    /// replaces was a number whose quantity the reader had to guess.
    #[test]
    fn the_live_line_labels_pump_cost_and_shows_a_distribution() {
        let line = HudSample {
            fps: 59.9,
            pump_p50_ms: 4.4,
            pump_p95_ms: 9.1,
            pump_max_ms: 21.0,
            samples: 60,
        }
        .line();

        assert!(line.contains("59.9 fps"), "{line}");
        assert!(
            line.contains("pump p50/p95/max 4.4/9.1/21.0 ms"),
            "the readout must name pump COST and show p50, p95 and max -- not a \
             threshold-crossing percentage over frame intervals: {line}"
        );
        assert!(
            !line.contains('%'),
            "no percentage belongs here; the 57.3%-over-budget artifact was exactly this \
             shape and was decided by jitter around its own threshold: {line}"
        );
    }

    fn feed(timing: &mut HudTiming, count: usize, interval_ms: u64, pump_ms: u64) {
        for _ in 0..count {
            timing.record(
                Duration::from_millis(interval_ms),
                Duration::from_millis(pump_ms),
            );
        }
    }

    #[test]
    fn a_barely_started_window_says_it_is_measuring_rather_than_inventing_a_number() {
        let mut timing = HudTiming::default();
        feed(&mut timing, 9, 16, 4);
        assert_eq!(
            timing.sample(Instant::now()),
            None,
            "nine samples is not a framerate"
        );
        feed(&mut timing, 1, 16, 4);
        assert!(timing.sample(Instant::now()).is_some());
    }

    #[test]
    fn fps_comes_from_the_interval_median_and_pump_stats_from_pump_cost() {
        let mut timing = HudTiming::default();
        // Two clearly different quantities, so a wire-up that read the wrong
        // one cannot produce this answer by coincidence.
        feed(&mut timing, 40, 20, 5);

        let sample = timing.sample(Instant::now()).expect("40 samples");
        assert!(
            (sample.fps - 50.0).abs() < 1e-9,
            "20 ms intervals are 50 fps, got {}",
            sample.fps
        );
        assert_eq!(sample.pump_p50_ms, 5.0);
        assert_eq!(sample.pump_max_ms, 5.0);
        assert_eq!(sample.samples, 40);
    }

    /// The distribution is the point: a run that is fast at p50 and terrible
    /// at max must not read as uniformly fast. This is the shape the
    /// discarded "% over budget" statistic could not express.
    #[test]
    fn a_lone_stall_shows_in_max_without_moving_the_median() {
        let mut timing = HudTiming::default();
        feed(&mut timing, 39, 16, 4);
        timing.record(Duration::from_millis(16), Duration::from_millis(45));

        let sample = timing.sample(Instant::now()).expect("40 samples");
        assert_eq!(sample.pump_p50_ms, 4.0);
        assert_eq!(sample.pump_max_ms, 45.0);
        assert!(
            sample.line().contains("4.0/") && sample.line().contains("45.0 ms"),
            "both ends of the distribution must be on the line: {}",
            sample.line()
        );
    }

    /// A single stalled frame must not be able to halve the reported rate for
    /// the next second -- the median is chosen over the mean for this.
    #[test]
    fn one_long_interval_does_not_drag_the_reported_rate() {
        let mut timing = HudTiming::default();
        feed(&mut timing, 39, 16, 4);
        timing.record(Duration::from_millis(400), Duration::from_millis(4));

        let sample = timing.sample(Instant::now()).expect("40 samples");
        assert!(
            (sample.fps - 62.5).abs() < 1e-9,
            "the median interval is still 16 ms, got {} fps",
            sample.fps
        );
    }

    #[test]
    fn the_window_stays_bounded_and_forgets_the_old_regime() {
        let mut timing = HudTiming::default();
        feed(&mut timing, 500, 33, 30);
        assert_eq!(timing.sample(Instant::now()).unwrap().samples, HUD_WINDOW);

        // A full window of the new regime must fully replace the old one.
        feed(&mut timing, HUD_WINDOW, 16, 4);
        let mut fresh = HudTiming::default();
        feed(&mut fresh, HUD_WINDOW, 16, 4);
        assert_eq!(
            timing.compute(),
            fresh.compute(),
            "the ring must hold only the newest HUD_WINDOW samples"
        );
    }

    #[test]
    fn the_displayed_sample_is_recomputed_on_its_own_cadence_not_every_frame() {
        let mut timing = HudTiming::default();
        feed(&mut timing, 30, 16, 4);
        let start = Instant::now();
        let first = timing.sample(start).expect("30 samples");

        // A different regime arrives, but before the refresh interval elapses
        // the HUD keeps showing what it last computed.
        feed(&mut timing, 30, 33, 30);
        assert_eq!(timing.sample(start + HUD_REFRESH / 2), Some(first));

        let refreshed = timing
            .sample(start + HUD_REFRESH)
            .expect("still has samples");
        assert_ne!(refreshed, first, "the refresh must actually recompute");
    }
}
