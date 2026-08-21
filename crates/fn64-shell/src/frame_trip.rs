//! Frame-hash tripwire: pin the first N presented frames, fail loudly if they
//! change.
//!
//! The parity corpus ([`fn64-render-conformance`]) diffs *synthetic* display
//! lists against RT64. Nothing guarded the frames the **game** actually
//! produces, so a renderer change could regress WM2000's picture while every
//! corpus case stayed green. This closes that gap with the cheapest possible
//! instrument: the shell already computes an FNV-1a `rgba_hash` of every
//! presented framebuffer, so pinning it adds no hashing, no clock, and no
//! work to the hot loop beyond a `Vec` push.
//!
//! `FN64_FRAME_TRIP=<file>` is read ONCE at boot (perf-method rule: no
//! per-frame env reads):
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
}

#[derive(Debug)]
pub struct FrameTrip {
    path: std::path::PathBuf,
    /// `None` in record mode, the pinned hashes in check mode.
    baseline: Option<Vec<u64>>,
    observed: Vec<u64>,
    capacity: usize,
}

impl FrameTrip {
    /// Read the environment once. Returns `None` when the tripwire is off,
    /// which is the default and costs one `var_os` at boot.
    pub fn from_env() -> Option<Self> {
        let path: std::path::PathBuf = std::env::var_os(ENV)?.into();
        Some(Self::at(path, DEFAULT_CAPACITY))
    }

    /// Split from `from_env` so tests can build one without touching the
    /// process environment (which is global and racy under a test harness).
    pub fn at(path: std::path::PathBuf, capacity: usize) -> Self {
        let baseline = std::fs::read_to_string(&path).ok().map(|text| parse(&text));
        Self {
            path,
            baseline,
            observed: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn is_checking(&self) -> bool {
        self.baseline.is_some()
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

        if let Some(baseline) = &self.baseline {
            // A baseline shorter than capacity bounds the run: compare what
            // was pinned, no more. Pinning 10 frames and checking 120 would
            // otherwise report a spurious mismatch at frame 10.
            if let Some(&expected) = baseline.get(index) {
                if expected != hash {
                    return Verdict::Mismatch {
                        index,
                        expected,
                        actual: hash,
                    };
                }
            }
            if self.observed.len() >= baseline.len().min(self.capacity).max(1) {
                return Verdict::Matched(self.observed.len());
            }
            return Verdict::Pending;
        }

        if self.observed.len() >= self.capacity {
            return Verdict::Recorded(self.observed.len())
        }
        Verdict::Pending
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
        t.baseline = Some(vec![0xaa, 0xbb]);
        assert!(t.is_checking());
        assert_eq!(t.observe(0xaa), Verdict::Pending);
        assert_eq!(t.observe(0xbb), Verdict::Matched(2));
    }

    /// The whole point of the tripwire: a changed frame is caught, and it is
    /// caught at the frame where it changed.
    #[test]
    fn the_first_differing_frame_is_the_one_reported() {
        let mut t = trip(8);
        t.baseline = Some(vec![0xaa, 0xbb, 0xcc]);
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
        t.baseline = Some(vec![0xaa]);
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
}
