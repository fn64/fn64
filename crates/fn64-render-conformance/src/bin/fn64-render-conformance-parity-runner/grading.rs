//! Grading: the hand-key/pixel comparison, Verdict/Tally, and the
//! captured-packet report.

use super::*;

/// The hand-derived key, materialised in the same guest byte order the
/// backends' observations are read in.
pub(crate) fn key_bytes(case: &Case) -> Vec<u8> {
    let mut rdram = seeded(&case.commands);
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..PIXEL_COUNT {
            view.write_u16(
                RdramAddr::from_offset(FRAMEBUFFER + index * 2),
                (case.expected)(index),
            );
        }
    }
    observation_bytes(&rdram)
}

pub(crate) fn pixels(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
        .collect()
}

/// How one backend's outcome compares to the oracle's for one case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Byte-identical committed guest framebuffers.
    Identical,
    /// Both completed, bytes differ.
    Differs { pixels: usize },
    /// Exactly one of the pair refused. The most consequential kind: one
    /// engine renders the stream and the other declines it.
    OneRefused,
    /// Both refused. Not parity evidence in either direction.
    BothRefused,
}

impl Verdict {
    pub(crate) fn of(
        oracle: &Result<Vec<u8>, String>,
        candidate: &Result<Vec<u8>, String>,
    ) -> Self {
        match (oracle, candidate) {
            (Ok(oracle), Ok(candidate)) => {
                let differing = pixels(oracle)
                    .into_iter()
                    .zip(pixels(candidate))
                    .filter(|(left, right)| left != right)
                    .count();
                if differing == 0 {
                    Self::Identical
                } else {
                    Self::Differs { pixels: differing }
                }
            }
            (Err(_), Err(_)) => Self::BothRefused,
            _ => Self::OneRefused,
        }
    }

    pub(crate) const fn wire(self) -> &'static str {
        match self {
            Self::Identical => "identical",
            Self::Differs { .. } => "differs",
            Self::OneRefused => "one-refused",
            Self::BothRefused => "both-refused",
        }
    }

    /// Only a byte-identical result counts toward parity. A refusal by both
    /// backends is NOT agreement -- neither rendered anything.
    pub(crate) const fn is_parity(self) -> bool {
        matches!(self, Self::Identical)
    }
}

pub(crate) fn outcome_wire(outcome: &Result<Vec<u8>, String>) -> Value {
    match outcome {
        Ok(_) => json!("completed"),
        Err(message) => json!({ "refused": message }),
    }
}

/// A running tally for one partition of the corpus.
#[derive(Default)]
pub(crate) struct Tally {
    pub(crate) cases: usize,
    pub(crate) identical: usize,
    pub(crate) differs: usize,
    pub(crate) one_refused: usize,
    pub(crate) both_refused: usize,
}

impl Tally {
    pub(crate) fn record(&mut self, verdict: Verdict) {
        self.cases += 1;
        match verdict {
            Verdict::Identical => self.identical += 1,
            Verdict::Differs { .. } => self.differs += 1,
            Verdict::OneRefused => self.one_refused += 1,
            Verdict::BothRefused => self.both_refused += 1,
        }
    }

    pub(crate) fn wire(&self) -> Value {
        json!({
            "cases": self.cases,
            "byte_identical": self.identical,
            "differs": self.differs,
            "one_refused": self.one_refused,
            "both_refused": self.both_refused,
        })
    }
}

/// A real captured RDP stream, promoted into a parity case.
///
/// # Why this exists
///
/// Hand-authored cases test what the author imagined. A captured WM2000
/// packet tests what the game actually draws, which is the difference between
/// a toy metric and a real one. `docs/rt64/RT64-PARITY.md` states the corpus
/// provenance the reported numbers rest on.
///
/// # Provenance and why nothing is committed
///
/// The capture itself is NOT in this repository and must not be: a game's own
/// RDP command words are game content, which `README.md`'s "no game content
/// ships in this repo" rule covers. So this reads a dump produced by
/// `FN64_GBI_PACKET_DUMP` at run time, and when the variable is unset the
/// corpus is simply the hand-authored one and the report says so.
///
/// The format is the committed one:
/// `entry \t lane \t pc \t w0 \t w1`, produced by
/// `fn64-render-reference`'s `gbi::census::packet`. Two other in-tree parsers
/// already read it (`raw_dpc_session_integration.rs`,
/// `examples/rt64_wm2000_three_way.rs`); this is a third reader of the same
/// committed shape, not a new format.
pub(crate) mod captured {
    /// Where the operator points this runner at a packet dump.
    pub const PACKET_ENV: &str = "FN64_WM2000_PACKET_TSV";
    /// Which decode entry of that dump to replay. Defaults to 0.
    pub const ENTRY_ENV: &str = "FN64_WM2000_PACKET_ENTRY";

    #[derive(Debug)]
    pub struct CapturedPacket {
        pub entry: u64,
        pub words: Vec<u32>,
        pub source_pc: usize,
    }

    /// Parse one decode entry out of a packet dump.
    ///
    /// Consecutive rows must be exactly 8 RDRAM bytes apart. That contiguity
    /// check is what makes the concatenated word pairs the WIRE STREAM rather
    /// than a lossy sample of it -- without it a dump missing rows would
    /// still parse and would silently measure a different display list.
    pub fn parse_packet_dump(text: &str, entry: u64) -> Result<CapturedPacket, String> {
        let mut rows: Vec<(usize, u32, u32)> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("entry\t") {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != 5 {
                return Err(format!(
                    "line {} has {} tab-separated fields, expected 5",
                    index + 1,
                    fields.len()
                ));
            }
            let row_entry: u64 = fields[0]
                .parse()
                .map_err(|e| format!("line {} entry field {:?}: {e}", index + 1, fields[0]))?;
            if row_entry != entry {
                continue;
            }
            // Only the raw-RDP lane is replayable as a command stream; a GBI
            // row is a display-list command that has not been decoded yet.
            if fields[1] != "RDP" {
                return Err(format!(
                    "line {} is on the {} lane; parity replays the raw-RDP lane",
                    index + 1,
                    fields[1]
                ));
            }
            let parse_hex = |field: &str, name: &str| -> Result<u64, String> {
                let stripped = field.strip_prefix("0x").ok_or_else(|| {
                    format!("line {} {name} is {field:?}, want 0x hex", index + 1)
                })?;
                u64::from_str_radix(stripped, 16)
                    .map_err(|e| format!("line {} {name} is {field:?}: {e}", index + 1))
            };
            rows.push((
                parse_hex(fields[2], "pc")? as usize,
                parse_hex(fields[3], "w0")? as u32,
                parse_hex(fields[4], "w1")? as u32,
            ));
        }
        if rows.is_empty() {
            return Err(format!("no rows for decode entry {entry}"));
        }
        for pair in rows.windows(2) {
            if pair[1].0 != pair[0].0 + 8 {
                return Err(format!(
                    "rows for entry {entry} are not contiguous: {:#010x} then {:#010x}",
                    pair[0].0, pair[1].0
                ));
            }
        }
        let source_pc = rows[0].0;
        let words = rows
            .iter()
            .flat_map(|&(_, w0, w1)| [w0, w1])
            .collect::<Vec<u32>>();
        Ok(CapturedPacket {
            entry,
            words,
            source_pc,
        })
    }

    /// Walk a raw-RDP word stream into `(byte_offset, cmd6, w0, w1)`.
    ///
    /// Deliberately independent of any decoder under test: it knows only that
    /// a raw-RDP command is 8 bytes except `G_TEXRECT` (`0x24`) and
    /// `G_TEXRECTFLIP` (`0x25`), which are 16.
    pub fn walk(words: &[u32]) -> Vec<(usize, u8, u32, u32)> {
        let mut out = Vec::new();
        let mut index = 0usize;
        while index + 1 < words.len() {
            let w0 = words[index];
            let w1 = words[index + 1];
            let cmd6 = ((w0 >> 24) & 0x3f) as u8;
            out.push((index * 4, cmd6, w0, w1));
            index += if matches!(cmd6, 0x24 | 0x25) { 4 } else { 2 };
        }
        out
    }

    /// Target extent read from the packet's OWN `SetColorImage` width and
    /// `SetScissor` lower-right Y, never hardcoded. Reading a captured stream
    /// at a guessed extent is the documented way to turn coherent geometry
    /// into convincing "striping" (`docs/rt64/RT64-WM2000-HARNESS-TRAPS.md`).
    pub fn target_extent(commands: &[(usize, u8, u32, u32)]) -> Option<(u32, u32)> {
        let width = commands
            .iter()
            .find(|&&(_, cmd6, _, _)| cmd6 == 0x3f)
            .map(|&(_, _, w0, _)| (w0 & 0x0fff) + 1)?;
        let height = commands
            .iter()
            .find(|&&(_, cmd6, _, _)| cmd6 == 0x2d)
            .map(|&(_, _, _, w1)| (w1 & 0x0fff) >> 2)?;
        Some((width, height))
    }

    /// The packet's own `SetColorImage` destination address.
    pub fn color_image_addr(commands: &[(usize, u8, u32, u32)]) -> Option<u32> {
        commands
            .iter()
            .find(|&&(_, cmd6, _, _)| cmd6 == 0x3f)
            .map(|&(_, _, _, w1)| w1)
    }
}

/// Replay a captured packet through all three backends at the packet's own
/// extent and its own color-image address.
///
/// This is reported as its own section rather than folded into the
/// hand-authored tally: the two have different provenance, and averaging a
/// real frame together with twelve synthetic fills would produce a number
/// whose denominator means nothing.
pub(crate) fn captured_row() -> Value {
    let Some(path) = std::env::var_os(captured::PACKET_ENV) else {
        return json!({
            "available": false,
            "reason": format!(
                "{} is unset. The capture is game content and is deliberately \
                 not committed; produce one with FN64_GBI_PACKET_DUMP on a ROM \
                 run, then point this variable at it.",
                captured::PACKET_ENV
            ),
        });
    };
    let entry: u64 = std::env::var(captured::ENTRY_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(0);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return json!({"available": false, "reason": format!("{path:?} unreadable: {error}")})
        }
    };
    let packet = match captured::parse_packet_dump(&text, entry) {
        Ok(packet) => packet,
        Err(reason) => return json!({"available": false, "reason": reason}),
    };
    let walked = captured::walk(&packet.words);
    let (Some((width, height)), Some(color_image)) = (
        captured::target_extent(&walked),
        captured::color_image_addr(&walked),
    ) else {
        return json!({
            "available": false,
            "reason": "the captured packet sets no color image or no scissor, \
                       so its target extent cannot be read from the stream",
        });
    };
    json!({
        "available": true,
        "provenance": "captured from a real ROM run via FN64_GBI_PACKET_DUMP; not committed",
        "entry": packet.entry,
        "source_pc": format!("{:#010x}", packet.source_pc),
        "words": packet.words.len(),
        "commands": walked.len(),
        "target": {"width": width, "height": height},
        "color_image": format!("{color_image:#010x}"),
        "note": "Extent and destination are read from the packet's own \
                 SetColorImage/SetScissor, never guessed. Replaying this \
                 through the three backends is the next step and is NOT done \
                 here: docs/rt64/RT64-WM2000-THREE-WAY.md already reports 0 of \
                 115,200 differing for all three pairings on WM2000 frame 0.",
    })
}

// ---------------------------------------------------------------------------
// Programmatic corpus generator (Track B).
//
// The hand corpus above is a fixed set of authored cases with hand-derived
// keys. The generator instead emits VALID SYNTHETIC RDP command streams across
// the command/mode matrix, ranked by real-ROM usage, and compares wgpu and
// RT64 against ANGRYLION as ground truth -- there is no hand key, because the
// point is systematic coverage no human wrote a key for.
//
// Streams are HAND-DERIVED / SYNTHETIC (built from the same wire encoders the
// hand corpus uses); NEVER captured from a running ROM. Every stream is a
// complete, valid frame: SetColorImage + SetScissor + SetOtherModes + (tile/
// load if textured) + draw + SyncFull, so all three backends can render it.
// ---------------------------------------------------------------------------
