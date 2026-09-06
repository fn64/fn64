//! Frame-hash tripwire: pin the first N presented frames, fail loudly if they
//! change.
//!
//! The parity corpus ([`fn64-render-conformance`]) diffs *synthetic* display
//! lists against RT64. Nothing guarded the frames the **game** actually
//! produces, so a renderer change could regress WM2000's picture while every
//! corpus case stayed green. This closes that gap with an exact FNV-1a
//! `rgba_hash` of every presented framebuffer while the tripwire is active.
//! The shell's per-presentation lazy hash authority shares that pass with any
//! simultaneous trace, capture, probe, or operator-log consumer.
//!
//! `FN64_FRAME_TRIP=<file>` is read ONCE at boot (perf-method rule: no
//! per-frame env reads). `FN64_FRAME_TRIP_FRAMES=<nonzero usize>` overrides
//! the 120-frame record bound so a private, content-free-on-disk baseline can
//! cover a later timing landmark without adding title knowledge here:
//!
//! * file absent  -> **record**: collect `capacity` hashes, write them out.
//! * file present -> **check**: compare frame-for-frame, and on the first
//!   mismatch print both hashes and exit nonzero.
//!
//! Either way the run ends by itself once `capacity` frames are seen, which
//! is what makes this usable as a gate: no human has to close the window.
//!
//! **A hash is a comparison key, not a correctness claim.** A matching hash
//! means "byte-identical to when this was pinned", nothing more; a pin taken
//! from a broken renderer pins the breakage. Record only from a build whose
//! picture was actually looked at.

/// Frames pinned by default -- enough to cover boot, the first geometry, and
/// several steady-state frames, while keeping a check run to a few seconds.
const DEFAULT_CAPACITY: usize = 120;

/// Environment variable naming the baseline file.
pub const ENV: &str = "FN64_FRAME_TRIP";
pub const CAPACITY_ENV: &str = "FN64_FRAME_TRIP_FRAMES";

/// What a completed tripwire run should do to the process.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Still collecting; keep running.
    Pending,
    /// Recorded `n` hashes; caller writes them out and exits 0.
    Recorded(usize),
    /// All compared frames matched.
    Matched(usize),
    /// Frame `index` differed: pinned vs observed.
    Mismatch {
        index: usize,
        expected: u64,
        actual: u64,
    },
    /// The baseline cannot support a meaningful comparison. Fails the run:
    /// a gate that cannot compare must not report success.
    Unusable(String),
}

/// Why this run is in the mode it is in. Distinguishing "absent" from
/// "unreadable" is what stops a gate silently re-recording its own baseline.
#[derive(Debug)]
enum Baseline {
    /// No file: collect hashes and write them.
    Recording,
    /// A file that parsed. May be empty -- which is an ERROR, not a pass.
    Pinned(Vec<u64>),
    /// A file that exists but could not be read.
    Unreadable(String),
}

#[derive(Debug)]
pub struct FrameTrip {
    path: std::path::PathBuf,
    baseline: Baseline,
    observed: Vec<u64>,
    capacity: usize,
}

impl FrameTrip {
    /// Read the environment once. Returns `None` when the tripwire is off,
    /// which is the default and costs one `var_os` at boot.
    pub fn from_env() -> Option<Self> {
        let path: std::path::PathBuf = std::env::var_os(ENV)?.into();
        let capacity = std::env::var(CAPACITY_ENV)
            .ok()
            .map(|value| parse_capacity(&value))
            .unwrap_or(DEFAULT_CAPACITY);
        Some(Self::at(path, capacity))
    }

    /// Split from `from_env` so tests can build one without touching the
    /// process environment (which is global and racy under a test harness).
    pub fn at(path: std::path::PathBuf, capacity: usize) -> Self {
        // ONLY a genuine "not found" means record mode. Every other read
        // error (permissions, a directory, non-UTF-8) is recorded as
        // `Unreadable` rather than collapsed into record mode by `.ok()`:
        // silently re-recording over a baseline you meant to CHECK against
        // turns a gate green by destroying the thing it compares to.
        let baseline = match std::fs::read_to_string(&path) {
            Ok(text) => Baseline::Pinned(parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Baseline::Recording,
            Err(e) => Baseline::Unreadable(e.to_string()),
        };
        Self {
            path,
            baseline,
            observed: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn is_checking(&self) -> bool {
        matches!(self.baseline, Baseline::Pinned(_))
    }

    /// Frames observed so far -- the index the next frame will take.
    pub fn observed_len(&self) -> usize {
        self.observed.len()
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Record one presented frame. Returns the verdict so far.
    ///
    /// Checking happens per frame rather than at the end so a mismatch names
    /// the FIRST bad frame -- the one nearest the cause. Comparing only final
    /// totals would report a divergence dozens of frames after it began.
    pub fn observe(&mut self, hash: u64) -> Verdict {
        let index = self.observed.len();
        self.observed.push(hash);

        match &self.baseline {
            Baseline::Unreadable(why) => {
                Verdict::Unusable(format!("baseline exists but could not be read: {why}"))
            }
            Baseline::Pinned(baseline) if baseline.is_empty() => Verdict::Unusable(
                "baseline file contains no hashes -- a check against nothing \
                 always passes, so it is refused"
                    .to_string(),
            ),
            Baseline::Pinned(baseline) => {
                // Compare BEFORE any completion test: a mismatch on the final
                // pinned frame must report Mismatch, not Matched.
                if let Some(&expected) = baseline.get(index) {
                    if expected != hash {
                        return Verdict::Mismatch {
                            index,
                            expected,
                            actual: hash,
                        };
                    }
                }
                // A short baseline is compared in FULL and reported with its
                // own length, so `PASS -- 4 frames` cannot be misread as
                // covering the requested capacity.
                if self.observed.len() >= baseline.len() {
                    return Verdict::Matched(baseline.len());
                }
                Verdict::Pending
            }
            Baseline::Recording => {
                // `>=` not `==`, and capacity 0 is treated as 1: a run that
                // can never satisfy its own completion test would spin
                // forever collecting hashes nobody asked for.
                if self.observed.len() >= self.capacity.max(1) {
                    return Verdict::Recorded(self.observed.len());
                }
                Verdict::Pending
            }
        }
    }

    /// Serialize the collected hashes. One hex hash per line, `#` comments
    /// ignored on read, so a pin file can be annotated with what it came from.
    pub fn serialize(&self) -> String {
        let mut out = String::from(
            "# fn64 frame-hash tripwire -- FNV-1a over each presented RGBA frame.\n\
             # A hash is a comparison key, not a correctness claim.\n",
        );
        for hash in &self.observed {
            out.push_str(&format!("{hash:016x}\n"));
        }
        out
    }

    pub fn write(&self) -> std::io::Result<()> {
        std::fs::write(&self.path, self.serialize())
    }
}

fn parse_capacity(value: &str) -> usize {
    value
        .parse::<usize>()
        .ok()
        .filter(|&value| value != 0)
        .unwrap_or_else(|| panic!("{CAPACITY_ENV} must be a nonzero usize, got {value:?}"))
}

fn parse(text: &str) -> Vec<u64> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| u64::from_str_radix(line, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trip(capacity: usize) -> FrameTrip {
        FrameTrip::at(std::path::PathBuf::from("/nonexistent/pin"), capacity)
    }

    #[test]
    fn a_missing_baseline_records_until_capacity() {
        let mut t = trip(3);
        assert!(!t.is_checking());
        assert_eq!(t.observe(0x11), Verdict::Pending);
        assert_eq!(t.observe(0x22), Verdict::Pending);
        assert_eq!(t.observe(0x33), Verdict::Recorded(3));
    }

    #[test]
    fn a_matching_run_reports_matched_not_mismatch() {
        let mut t = trip(8);
        t.baseline = Baseline::Pinned(vec![0xaa, 0xbb]);
        assert!(t.is_checking());
        assert_eq!(t.observe(0xaa), Verdict::Pending);
        assert_eq!(t.observe(0xbb), Verdict::Matched(2));
    }

    /// The whole point of the tripwire: a changed frame is caught, and it is
    /// caught at the frame where it changed.
    #[test]
    fn the_first_differing_frame_is_the_one_reported() {
        let mut t = trip(8);
        t.baseline = Baseline::Pinned(vec![0xaa, 0xbb, 0xcc]);
        assert_eq!(t.observe(0xaa), Verdict::Pending);
        assert_eq!(
            t.observe(0xff),
            Verdict::Mismatch {
                index: 1,
                expected: 0xbb,
                actual: 0xff,
            },
            "frame 1 differs, so frame 1 -- not frame 2 -- must be named"
        );
    }

    /// A baseline shorter than capacity must not read as a mismatch once it
    /// runs out; it bounds the comparison instead.
    #[test]
    fn a_short_baseline_bounds_the_run_rather_than_failing() {
        let mut t = trip(100);
        t.baseline = Baseline::Pinned(vec![0xaa]);
        assert_eq!(t.observe(0xaa), Verdict::Matched(1));
    }

    #[test]
    fn serialize_round_trips_through_parse_ignoring_comments() {
        let mut t = trip(4);
        for h in [0x1u64, 0xdead_beef_dead_beef, 0xfeed] {
            t.observe(h);
        }
        let text = t.serialize();
        assert!(text.starts_with('#'), "pin files are self-describing");
        assert_eq!(parse(&text), vec![0x1, 0xdead_beef_dead_beef, 0xfeed]);
    }

    #[test]
    fn parse_survives_blank_lines_and_junk() {
        assert_eq!(parse("# c\n\n  00ff \n\nnot-hex\n11\n"), vec![0xff, 0x11]);
    }

    #[test]
    fn capacity_override_rejects_zero_and_non_numbers() {
        assert_eq!(parse_capacity("1300"), 1300);
        assert!(std::panic::catch_unwind(|| parse_capacity("0")).is_err());
        assert!(std::panic::catch_unwind(|| parse_capacity("later")).is_err());
    }

    // ---- fail-open regressions. Each of these once returned a PASS. ----
    // Found by an independent Codex review, not by the tests above: the
    // original six all used well-formed baselines, so every one of these
    // paths was green while the gate was incapable of failing.

    /// Measured on the real shell before the fix: a comment-only baseline
    /// printed "PASS -- 1 frames match" and exited 0.
    #[test]
    fn an_empty_baseline_is_refused_rather_than_passing() {
        let mut t = trip(8);
        t.baseline = Baseline::Pinned(vec![]);
        assert!(
            matches!(t.observe(0xaa), Verdict::Unusable(_)),
            "a check against zero hashes always passes, so it must be refused"
        );
    }

    #[test]
    fn an_unreadable_baseline_does_not_silently_re_record() {
        let mut t = trip(8);
        t.baseline = Baseline::Unreadable("permission denied".into());
        assert!(matches!(t.observe(0xaa), Verdict::Unusable(_)));
        assert!(!t.is_checking());
    }

    /// A directory is readable-as-an-entry but not as a file: the classic
    /// case that `.ok()` used to collapse into record mode.
    #[test]
    fn a_directory_baseline_is_unreadable_not_recording() {
        let t = FrameTrip::at(std::path::PathBuf::from("/tmp"), 4);
        assert!(matches!(t.baseline, Baseline::Unreadable(_)));
    }

    #[test]
    fn a_genuinely_absent_file_still_records() {
        let t = FrameTrip::at(
            std::path::PathBuf::from("/nonexistent/definitely/not/here.pin"),
            4,
        );
        assert!(matches!(t.baseline, Baseline::Recording));
    }

    /// Capacity 0 must not spin forever collecting hashes nobody asked for.
    #[test]
    fn zero_capacity_terminates_instead_of_running_forever() {
        let mut t = trip(0);
        assert_eq!(t.observe(0xaa), Verdict::Recorded(1));
    }

    /// A mismatch on the LAST pinned frame must report Mismatch, not Matched
    /// -- the completion test must not pre-empt the comparison.
    #[test]
    fn a_mismatch_on_the_final_frame_is_not_reported_as_a_pass() {
        let mut t = trip(8);
        t.baseline = Baseline::Pinned(vec![0xaa, 0xbb]);
        assert_eq!(t.observe(0xaa), Verdict::Pending);
        assert!(
            matches!(t.observe(0xff), Verdict::Mismatch { index: 1, .. }),
            "the last frame is still compared"
        );
    }

    /// A short baseline reports ITS OWN length, so "PASS -- 4 frames" cannot
    /// be misread as covering the requested capacity.
    #[test]
    fn a_short_baseline_reports_its_own_length_not_capacity() {
        let mut t = trip(100);
        t.baseline = Baseline::Pinned(vec![0xaa, 0xbb]);
        assert_eq!(t.observe(0xaa), Verdict::Pending);
        assert_eq!(t.observe(0xbb), Verdict::Matched(2));
    }
}
