//! F2 screenshot capture: write the game's current RGBA8888 frame to a PNG.
//!
//! The pieces that decide *where* a file lands and *what it is called* are
//! pure functions here so they are unit-testable without a window; the shell's
//! `window_event` handler only supplies the frame and reports the outcome.
//! `main.rs`'s `present()` already unpacks the VI framebuffer into its `rgba`
//! scratch buffer immediately before blitting, so a capture is a copy of that
//! buffer -- no readback, no extra conversion, no second PNG encoder (this
//! module calls `fn64_render_reference::png_dump::encode_rgba8`).
//!
//! ## What the image contains
//!
//! The **game frame only**, never the settings overlay. That is a property of
//! where the bytes come from rather than a filter applied afterwards:
//! `present()` fills `rgba` from rdram, copies it into the pixels surface, and
//! only then does `Overlay::render_over` draw egui onto the GPU surface. The
//! overlay never touches `rgba`, so capturing it cannot pick up UI chrome.
//! That is also the behavior a player wants -- a screenshot of the game, not
//! of the menu they happened to leave open.
//!
//! ## Where the files land
//!
//! `FN64_SCREENSHOT_DIR` if set, otherwise a `screenshots/` directory beside
//! the process's working directory. Both are discoverable without reading
//! source: the shell prints the absolute path of every file it writes, and
//! prints the directory it tried when a write fails.
//!
//! ## Failure is loud but never fatal
//!
//! `AGENTS.md` forbids silent no-ops, and a screenshot that fails silently is
//! exactly that. It equally must not kill a live game session -- panicking the
//! winit event handler over a full disk would lose the player's progress for
//! no reason. Both hold at once because the failure is *reported* rather than
//! *swallowed or escalated*: [`capture`] returns a `Result`, and its caller
//! prints the concrete error (path included) to stderr and keeps running. The
//! error type is not `#[must_use]`-dodgeable -- `capture` hands back a
//! `Result` the caller has to handle -- so a future edit cannot reintroduce a
//! silent path without deleting a visible `eprintln!`.

use std::path::{Path, PathBuf};

/// Why a capture could not be written. Each variant carries what the operator
/// needs to fix it, so `Display` alone is an actionable message.
#[derive(Debug)]
pub enum CaptureError {
    /// `present()` has not run yet, so the RGBA scratch buffer holds no frame.
    /// Distinguished from a legitimately black frame: an all-zero `rgba` is a
    /// valid image, and writing it would be a lie about what was on screen.
    NoFrameYet,
    /// The buffer length disagrees with the dimensions it claims. A bug, not
    /// an environment problem, but still not worth killing a session over.
    MalformedFrame {
        width: usize,
        height: usize,
        len: usize,
    },
    /// The output directory could not be created.
    CreateDir {
        dir: PathBuf,
        source: std::io::Error,
    },
    /// The PNG itself could not be written.
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFrameYet => write!(
                f,
                "no frame has been presented yet -- nothing to capture (wait for the game to \
                 render its first VI swap)"
            ),
            Self::MalformedFrame { width, height, len } => write!(
                f,
                "frame buffer is {len} bytes but {width}x{height} RGBA8888 needs {} -- refusing \
                 to encode a malformed image",
                width * height * 4
            ),
            Self::CreateDir { dir, source } => {
                write!(f, "could not create {}: {source}", dir.display())
            }
            Self::Write { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateDir { source, .. } | Self::Write { source, .. } => Some(source),
            Self::NoFrameYet | Self::MalformedFrame { .. } => None,
        }
    }
}

/// Env var naming the directory screenshots are written to.
pub const DIR_ENV: &str = "FN64_SCREENSHOT_DIR";

/// The directory screenshots land in, given the value of [`DIR_ENV`].
///
/// `None`/empty selects the default `screenshots/` under the working
/// directory. The value is used verbatim otherwise, including a relative path
/// -- resolving it against the CWD is the shell's normal behavior for every
/// other path it takes, and an operator who wants an absolute location can
/// give one.
pub fn resolve_dir(env_value: Option<&str>) -> PathBuf {
    match env_value {
        Some(raw) if !raw.trim().is_empty() => PathBuf::from(raw.trim()),
        _ => PathBuf::from("screenshots"),
    }
}

/// Build a screenshot filename from a wall-clock instant and a within-session
/// sequence number.
///
/// `unix_millis` is milliseconds since the Unix epoch; `seq` distinguishes two
/// captures that land inside the same millisecond. Millisecond resolution
/// alone already separates two human keypresses, but a held key, a scripted
/// press, or a coarse platform clock can repeat a millisecond, so the sequence
/// number is what actually carries the uniqueness guarantee -- it is
/// monotonic for the life of the process and never reused.
///
/// The timestamp is rendered as local-agnostic UTC, spelled out rather than
/// left as an epoch integer, so `ls` sorts chronologically and a human can
/// read the name.
pub fn file_name(unix_millis: u64, seq: u64) -> String {
    let (y, mo, d, h, mi, s, ms) = civil_from_unix_millis(unix_millis);
    format!("fn64-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{ms:03}-{seq:04}.png")
}

/// Split Unix milliseconds into `(year, month, day, hour, minute, second,
/// millisecond)` in UTC.
///
/// Uses Howard Hinnant's `civil_from_days` algorithm (public domain, from his
/// `chrono`-compatible date algorithms note) so no date crate is needed. The
/// era arithmetic handles leap years and century rules exactly; the tests pin
/// it against independently known instants including a leap day.
fn civil_from_unix_millis(unix_millis: u64) -> (u64, u64, u64, u64, u64, u64, u64) {
    let secs = unix_millis / 1000;
    let ms = unix_millis % 1000;
    let days = secs / 86_400;
    let secs_of_day = secs % 86_400;
    let (h, mi, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );

    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097; // day of era, [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, h, mi, s, ms)
}

/// Milliseconds since the Unix epoch, or 0 if the host clock is before it.
pub fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Hands out the never-reused sequence numbers [`file_name`] needs, and
/// remembers whether a frame has been presented at all.
#[derive(Debug, Default)]
pub struct Screenshotter {
    seq: u64,
}

impl Screenshotter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Next sequence number. Monotonic; saturating rather than wrapping so a
    /// pathological session cannot alias an earlier file name.
    pub fn next_seq(&mut self) -> u64 {
        let n = self.seq;
        self.seq = self.seq.saturating_add(1);
        n
    }

    /// Name the next diagnostic frame dump with this same session-wide
    /// capture authority. Frame dumping does not require a hash tripwire, so
    /// its chronology cannot be derived from a tripwire observation count.
    pub fn next_frame_dump_file_name(&mut self, rgba_hash: u64) -> String {
        let seq = self.next_seq();
        format!("frame-{seq:04}-{rgba_hash:016x}.png")
    }
}

/// Encode `rgba` and write it into `dir`, returning the path written.
///
/// `has_frame` is the caller's assertion that `rgba` holds a presented frame;
/// `false` yields [`CaptureError::NoFrameYet`] rather than a black PNG.
pub fn capture(
    dir: &Path,
    file: &str,
    width: usize,
    height: usize,
    rgba: &[u8],
    has_frame: bool,
) -> Result<PathBuf, CaptureError> {
    if !has_frame {
        return Err(CaptureError::NoFrameYet);
    }
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        return Err(CaptureError::MalformedFrame {
            width,
            height,
            len: rgba.len(),
        });
    }
    std::fs::create_dir_all(dir).map_err(|source| CaptureError::CreateDir {
        dir: dir.to_path_buf(),
        source,
    })?;
    let path = dir.join(file);
    // The shared in-repo encoder, not a second one: dependency-free PNG for
    // exactly this kind of dump (see its module doc for the size tradeoff --
    // stored DEFLATE, so a 480x240 capture is a flat ~461 KB).
    fn64_render_reference::png_dump::write_png(&path, width as u32, height as u32, rgba).map_err(
        |source| CaptureError::Write {
            path: path.clone(),
            source,
        },
    )?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dir_is_screenshots_under_the_working_directory() {
        // Asserted against the OS string the user actually types into `cd`,
        // not against `PathBuf::from("screenshots")`. The first draft used the
        // latter and a rename mutation survived, because the expectation was a
        // second copy of the same literal and the mutation edited both. The
        // name is part of the user-facing contract, so it is pinned as text.
        assert_eq!(resolve_dir(None).as_os_str(), "screenshots");
        assert_eq!(resolve_dir(Some("")).as_os_str(), "screenshots");
        // A var set to whitespace is an operator mistake, not a request for a
        // directory literally named " ".
        assert_eq!(resolve_dir(Some("   ")).as_os_str(), "screenshots");
        // Relative, so it lands beside the working directory rather than at
        // the filesystem root.
        assert!(resolve_dir(None).is_relative());
    }

    #[test]
    fn env_override_wins_and_is_trimmed() {
        assert_eq!(resolve_dir(Some("/tmp/shots")), PathBuf::from("/tmp/shots"));
        assert_eq!(
            resolve_dir(Some("  /tmp/shots\n")),
            PathBuf::from("/tmp/shots")
        );
        // Relative overrides are honored verbatim, not rewritten.
        assert_eq!(resolve_dir(Some("out/pics")), PathBuf::from("out/pics"));
    }

    #[test]
    fn file_name_renders_the_instant_two_independent_ways() {
        // 2026-08-17T12:34:56.789Z. Derived twice and reconciled: once by the
        // civil algorithm under test, once from an independently computed
        // epoch-seconds literal.
        //
        // Independent derivation of the seconds value: days from 1970-01-01
        // to 2026-01-01 is 56 years * 365 + 14 leap days (1972..=2024 every
        // 4th year is 14 of them; 2000 is a leap year under the 400 rule, so
        // no century correction applies in this span) = 20440 + 14 = 20454.
        // 2026-01-01 to 2026-08-17: 31+28+31+30+31+30+31 = 212 days through
        // July, +16 days into August = 228. Total 20682 days.
        let days = 20_454u64 + 228;
        let secs = days * 86_400 + 12 * 3600 + 34 * 60 + 56;
        let millis = secs * 1000 + 789;
        assert_eq!(
            file_name(millis, 0),
            "fn64-20260817-123456-789-0000.png",
            "civil conversion disagrees with the hand-derived epoch instant"
        );
        // And the components round-trip back out of the algorithm.
        assert_eq!(
            civil_from_unix_millis(millis),
            (2026, 8, 17, 12, 34, 56, 789)
        );
    }

    #[test]
    fn civil_conversion_handles_the_epoch_and_a_leap_day() {
        assert_eq!(civil_from_unix_millis(0), (1970, 1, 1, 0, 0, 0, 0));
        // 2024-02-29T00:00:00Z = 19782 days after the epoch. Independently:
        // 1970..2024 is 54 years * 365 = 19710, plus leap days for
        // 1972..=2020 stepping 4 = 13, = 19723 for 2024-01-01; +31 days of
        // January = 19754 for 2024-02-01; +28 = 19782 for 2024-02-29.
        let leap_day = 19_782u64 * 86_400 * 1000;
        assert_eq!(civil_from_unix_millis(leap_day), (2024, 2, 29, 0, 0, 0, 0));
        // The day after a leap day is March 1st, not February 30th.
        let next = leap_day + 86_400 * 1000;
        assert_eq!(civil_from_unix_millis(next), (2024, 3, 1, 0, 0, 0, 0));
        // And a non-leap century boundary: 1900 is not a leap year, but the
        // epoch postdates it, so pin 2100 instead -- 2100 is divisible by 100
        // and not by 400, so 2100-03-01 is the day after 2100-02-28 with no
        // February 29th between them. The first draft of this test asserted
        // 4_107_542_400 here from a hand derivation; the implementation
        // disagreed, and the implementation was right -- that value is
        // 2100-03-01, and 2100-02-28 is 86_400 earlier. Corrected against an
        // independent source rather than by adjusting the code under test.
        let feb28_2100 = 4_107_456_000u64 * 1000; // 2100-02-28T00:00:00Z
        assert_eq!(
            civil_from_unix_millis(feb28_2100),
            (2100, 2, 28, 0, 0, 0, 0)
        );
        assert_eq!(
            civil_from_unix_millis(feb28_2100 + 86_400 * 1000),
            (2100, 3, 1, 0, 0, 0, 0)
        );
    }

    #[test]
    fn two_captures_in_the_same_millisecond_do_not_collide() {
        let mut shots = Screenshotter::new();
        let same_instant = 1_760_000_000_000u64;
        let a = file_name(same_instant, shots.next_seq());
        let b = file_name(same_instant, shots.next_seq());
        assert_ne!(a, b, "same-millisecond captures must not share a file name");
        // Both still carry the identical timestamp -- the sequence number is
        // what separates them, which is the guarantee being claimed.
        assert_eq!(
            a.trim_end_matches("-0000.png"),
            b.trim_end_matches("-0001.png")
        );
    }

    #[test]
    fn sequence_numbers_are_monotonic_and_never_reused() {
        let mut shots = Screenshotter::new();
        let seen: Vec<u64> = (0..64).map(|_| shots.next_seq()).collect();
        for pair in seen.windows(2) {
            assert!(pair[1] > pair[0], "sequence must strictly increase");
        }
        let unique: std::collections::BTreeSet<u64> = seen.iter().copied().collect();
        assert_eq!(unique.len(), seen.len(), "sequence numbers must be unique");
    }

    #[test]
    fn frame_dump_names_advance_without_a_tripwire_and_repeated_pixels_do_not_overwrite() {
        let mut shots = Screenshotter::new();
        assert_eq!(
            shots.next_frame_dump_file_name(0x1234),
            "frame-0000-0000000000001234.png"
        );
        assert_eq!(
            shots.next_frame_dump_file_name(0x1234),
            "frame-0001-0000000000001234.png"
        );
    }

    #[test]
    fn names_sort_chronologically_as_plain_strings() {
        // The zero-padded fixed-width layout is the reason `ls` is useful.
        let earlier = file_name(1_760_000_000_000, 0);
        let later = file_name(1_760_000_001_000, 0);
        assert!(earlier < later, "{earlier} should sort before {later}");
        // Padding holds across a digit-count change in the sequence number.
        let s9 = file_name(1_760_000_000_000, 9);
        let s10 = file_name(1_760_000_000_000, 10);
        assert!(s9 < s10, "{s9} should sort before {s10}");
    }

    #[test]
    fn capture_without_a_presented_frame_is_an_error_not_a_black_png() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rgba = vec![0u8; 4 * 4 * 4];
        let err = capture(dir.path(), "x.png", 4, 4, &rgba, false)
            .expect_err("no-frame capture must fail");
        assert!(matches!(err, CaptureError::NoFrameYet));
        // Nothing was written, and the directory was not even created.
        assert!(!dir.path().join("x.png").exists());
    }

    #[test]
    fn capture_writes_a_real_png_and_reports_its_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Two levels deep: create_dir_all, not create_dir.
        let nested = dir.path().join("a").join("b");
        let rgba: Vec<u8> = (0..(4 * 3 * 4)).map(|i| (i % 256) as u8).collect();
        let path = capture(&nested, "shot.png", 4, 3, &rgba, true).expect("capture");
        assert_eq!(path, nested.join("shot.png"));
        let bytes = std::fs::read(&path).expect("written file is readable");
        assert_eq!(
            &bytes[0..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "written file must carry the PNG signature"
        );
        // The dimensions in IHDR are the ones we asked for, not the defaults.
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 3);
    }

    #[test]
    fn malformed_frame_is_rejected_before_the_encoder_asserts() {
        // encode_rgba8 asserts on a length mismatch, which in the shell would
        // be a panic inside the winit event handler -- the exact thing this
        // guard exists to prevent. Checked here rather than trusted.
        let dir = tempfile::tempdir().expect("tempdir");
        let rgba = vec![0u8; 7];
        let err =
            capture(dir.path(), "x.png", 4, 3, &rgba, true).expect_err("length mismatch must fail");
        match err {
            CaptureError::MalformedFrame { width, height, len } => {
                assert_eq!((width, height, len), (4, 3, 7));
            }
            other => panic!("expected MalformedFrame, got {other:?}"),
        }
        // A zero dimension is rejected too (0x0 would otherwise "match").
        assert!(matches!(
            capture(dir.path(), "x.png", 0, 0, &[], true),
            Err(CaptureError::MalformedFrame { .. })
        ));
    }

    #[test]
    fn write_failure_names_the_path_it_could_not_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A directory occupying the target file name makes File::create fail
        // without needing a permission or full-disk simulation.
        let blocked = dir.path().join("blocked.png");
        std::fs::create_dir(&blocked).expect("occupy the name with a directory");
        let rgba = vec![0u8; 2 * 2 * 4];
        let err = capture(dir.path(), "blocked.png", 2, 2, &rgba, true)
            .expect_err("writing over a directory must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("blocked.png"),
            "error must name the path it failed on, got: {msg}"
        );
        assert!(matches!(err, CaptureError::Write { .. }));
    }

    #[test]
    fn every_error_renders_a_nonempty_actionable_message() {
        // A Display that returns "" would satisfy "we logged something" while
        // being a silent no-op in practice.
        for err in [
            CaptureError::NoFrameYet,
            CaptureError::MalformedFrame {
                width: 4,
                height: 3,
                len: 7,
            },
            CaptureError::CreateDir {
                dir: PathBuf::from("/nope"),
                source: std::io::Error::other("boom"),
            },
            CaptureError::Write {
                path: PathBuf::from("/nope/x.png"),
                source: std::io::Error::other("boom"),
            },
        ] {
            let msg = err.to_string();
            assert!(msg.len() > 20, "error message too terse: {msg:?}");
        }
    }
}
