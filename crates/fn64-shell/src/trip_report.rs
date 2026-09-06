//! How a settled frame-tripwire [`Verdict`](crate::frame_trip::Verdict) is
//! reported and what it does to the process exit status.
//!
//! This is the decision, not the doing. `main.rs` still performs the two
//! side effects a verdict implies -- writing the baseline file for a
//! `Recorded` verdict, and printing -- because both are I/O. What moves here
//! is the pure mapping from (verdict, baseline path, write outcome) to the
//! line an operator reads and the code the process exits with, which is the
//! part a test can hold still.
//!
//! **The exit code is the contract.** Tripwire runs are gates, so their
//! verdict must reach a CI script as a status. Two failure modes matter and
//! are pinned below:
//!
//! * A gate that cannot compare must not report success. `Unusable` exits
//!   nonzero -- a comment-only baseline was once measured reporting
//!   "PASS -- 1 frames match".
//! * A baseline that fails to WRITE is a failed record run, not a successful
//!   one, so the write error also exits nonzero.
//!
//! A matching hash means "byte-identical to when this was pinned", nothing
//! more; a differing hash localises the frame but does not itself say which
//! picture is correct. The wording below says so, and the tests keep it
//! saying so.

use crate::frame_trip::Verdict;

/// Which stream a report line belongs on. Success goes to stdout, failure to
/// stderr, so a CI log that captures only one of the two still sees the
/// interesting half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// What the caller should print, and with what status to leave the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub stream: Stream,
    pub message: String,
    pub code: i32,
}

impl Report {
    fn pass(message: String) -> Self {
        Self { stream: Stream::Stdout, message, code: 0 }
    }

    fn fail(message: String) -> Self {
        Self { stream: Stream::Stderr, message, code: 1 }
    }
}

/// Report a `Recorded` verdict, given how the baseline write went.
///
/// Split from [`report`] because the write is the caller's I/O: `main.rs`
/// calls `FrameTrip::write()` and hands the outcome here as a message, so
/// this function stays pure while still deciding both halves of the outcome.
pub fn report_recorded(frames: usize, path: &str, write_error: Option<&str>) -> Report {
    match write_error {
        None => Report::pass(format!(
            "[fn64-shell] frame tripwire: recorded {frames} frame hashes to {path}"
        )),
        // A record run that could not persist its baseline recorded nothing
        // usable. Failing here is what stops the next check run from
        // comparing against a file that was never written.
        Some(error) => Report::fail(format!(
            "[fn64-shell] frame tripwire: FAILED to write {path}: {error}"
        )),
    }
}

/// Report any settled verdict other than `Recorded`.
///
/// # Panics
///
/// On [`Verdict::Pending`], which is never stored: the shell keeps pumping
/// while a verdict is pending and only reaches here once one has settled.
/// On [`Verdict::Recorded`], which needs the write outcome -- use
/// [`report_recorded`].
pub fn report(verdict: &Verdict, path: &str) -> Report {
    match verdict {
        Verdict::Pending => unreachable!("Pending is never stored"),
        Verdict::Recorded(_) => {
            unreachable!("Recorded needs its write outcome -- use report_recorded")
        }
        Verdict::Matched(frames) => Report::pass(format!(
            "[fn64-shell] frame tripwire: PASS -- {frames} frames match {path}"
        )),
        // Fails the run. A gate that cannot compare must not report success:
        // a comment-only baseline was measured reporting "PASS -- 1 frames
        // match".
        Verdict::Unusable(why) => Report::fail(format!(
            "[fn64-shell] frame tripwire: UNUSABLE -- {why} ({path})"
        )),
        Verdict::Mismatch { index, expected, actual } => Report::fail(format!(
            "[fn64-shell] frame tripwire: FAIL at frame {index} -- pinned \
             {expected:016x}, got {actual:016x} (baseline {path}). A differing \
             hash localises the frame; it does not itself say which picture \
             is correct."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matched_passes_on_stdout_with_the_frame_count_and_baseline() {
        assert_eq!(
            report(&Verdict::Matched(120), "/tmp/wm2000.trip"),
            Report {
                stream: Stream::Stdout,
                message: "[fn64-shell] frame tripwire: PASS -- 120 frames match \
                          /tmp/wm2000.trip"
                    .to_string(),
                code: 0,
            }
        );
    }

    /// The regression this whole type exists to prevent: a baseline that
    /// cannot support a comparison must NOT read as a pass. Assert the code,
    /// the stream, and the absence of the word PASS.
    #[test]
    fn unusable_fails_and_never_reads_as_a_pass() {
        let got = report(
            &Verdict::Unusable("baseline holds 0 hashes (comments only)".to_string()),
            "/tmp/empty.trip",
        );
        assert_eq!(got.code, 1);
        assert_eq!(got.stream, Stream::Stderr);
        assert!(got.message.contains("UNUSABLE"));
        assert!(!got.message.contains("PASS"));
        // The reason must survive into the message: "UNUSABLE" alone does not
        // tell an operator what to fix.
        assert!(got.message.contains("baseline holds 0 hashes (comments only)"));
        assert!(got.message.contains("/tmp/empty.trip"));
    }

    /// Both hashes are printed zero-padded to 16 hex digits so two lines can
    /// be eyeballed column-wise; a leading-zero hash must not shrink.
    #[test]
    fn mismatch_prints_both_hashes_zero_padded_to_sixteen_digits() {
        let got = report(
            &Verdict::Mismatch { index: 7, expected: 0x00ab, actual: 0xdead_beef },
            "base.trip",
        );
        assert_eq!(got.code, 1);
        assert_eq!(got.stream, Stream::Stderr);
        assert!(got.message.contains("FAIL at frame 7"));
        assert!(got.message.contains("pinned 00000000000000ab"));
        assert!(got.message.contains("got 00000000deadbeef"));
    }

    /// A hash is a comparison key, not a correctness claim. The mismatch
    /// message must keep saying so -- it is the sentence that stops a reader
    /// concluding the pinned picture was the right one.
    #[test]
    fn mismatch_disclaims_that_it_says_which_picture_is_correct() {
        let got = report(
            &Verdict::Mismatch { index: 0, expected: 1, actual: 2 },
            "base.trip",
        );
        assert!(got
            .message
            .contains("does not itself say which picture is correct"));
    }

    #[test]
    fn recorded_passes_when_the_baseline_was_written() {
        assert_eq!(
            report_recorded(120, "/tmp/new.trip", None),
            Report {
                stream: Stream::Stdout,
                message: "[fn64-shell] frame tripwire: recorded 120 frame hashes to \
                          /tmp/new.trip"
                    .to_string(),
                code: 0,
            }
        );
    }

    /// A record run whose baseline never reached disk produced nothing the
    /// next check run can use, so it fails rather than reporting a recording
    /// that does not exist.
    #[test]
    fn recorded_fails_when_the_baseline_write_failed() {
        let got = report_recorded(120, "/ro/new.trip", Some("Read-only file system (os error 30)"));
        assert_eq!(got.code, 1);
        assert_eq!(got.stream, Stream::Stderr);
        assert!(got.message.contains("FAILED to write /ro/new.trip"));
        assert!(got.message.contains("Read-only file system (os error 30)"));
        assert!(!got.message.contains("recorded 120 frame hashes"));
    }

    /// Exactly one verdict shape exits 0 on the failure side of the split:
    /// none. Every non-`Matched`, non-clean-`Recorded` outcome is nonzero, so
    /// a gate cannot pass by accident.
    #[test]
    fn only_matched_and_clean_recorded_exit_zero() {
        let zero = [
            report(&Verdict::Matched(1), "p").code,
            report_recorded(1, "p", None).code,
        ];
        let nonzero = [
            report(&Verdict::Unusable("why".to_string()), "p").code,
            report(&Verdict::Mismatch { index: 0, expected: 0, actual: 1 }, "p").code,
            report_recorded(1, "p", Some("io")).code,
        ];
        assert_eq!(zero, [0, 0]);
        assert!(nonzero.iter().all(|&c| c != 0), "{nonzero:?}");
    }

    #[test]
    #[should_panic(expected = "Pending is never stored")]
    fn pending_is_not_a_reportable_verdict() {
        let _ = report(&Verdict::Pending, "p");
    }

    #[test]
    #[should_panic(expected = "use report_recorded")]
    fn recorded_routed_through_the_wrong_entry_point_panics() {
        let _ = report(&Verdict::Recorded(1), "p");
    }
}
