//! Backend-neutral inspection of the public RDP completion command.
//!
//! Widths follow SGI *RDP Command Summary* Table 11, extended over the
//! command ids Table 11 leaves unlisted by the N64brew RDP command table,
//! which documents them as *No Operation*. Triangle coefficient payloads are
//! skipped structurally so an arbitrary coefficient byte cannot impersonate a
//! command opcode.
//!
//! Widening the map does not make it permissive: an opcode with no documented
//! width is still rejected loudly rather than advanced by a guessed stride,
//! because a wrong stride resynchronizes the scan onto payload bytes and lets
//! a coefficient impersonate `G_RDPFULLSYNC`.

use crate::{DpFullSyncStatus, RenderError};

const G_NOOP: u8 = 0x00;
/// Highest RDP command id in the low no-operation block. The N64brew RDP
/// command table assigns *No Operation* to every id in `0x00..=0x07` (and
/// again at `0x10..=0x23` and `0x31`); each stalls the pipeline for one cycle
/// and occupies exactly one 64-bit command word. `G_NOOP` is the `0x00` member
/// of that same block, spelled with its public GBI name because `gDPNoOp` /
/// `gDPNoOpTag` emit it.
///
/// The same table marks `0x10..=0x23` and `0x31` No Operation too, and those
/// are deliberately NOT accepted here. Nothing observed emits them, the
/// reference decoder already pins `0x10` as an unrecognized-command rejection
/// (`gbi/tests/group5.rs`, `raw_rdp_unknown_opcode_records_returned_error`),
/// and `0x10..=0x23` overlaps the wire bytes a mis-synchronized scan is most
/// likely to land on. Accepting an id costs the scanner its ability to
/// notice it has lost the command boundary, so ids are widened when a real
/// microcode emits them, not because a table lists them.
const RDP_LOW_NOOP_END: u8 = 0x07;
/// The one id carved out of that excluded block, on exactly the evidence the
/// rule above demands: a real microcode emits it.
///
/// WM2000 writes `0x1f` to terminate every graphics submission -- 219
/// occurrences across 383 VI fields, present in 218/218 frames with recorded
/// deltas, measured over two byte-identical runs of the real ROM
/// (`docs/RT64-WM2000-CENSUS.md` §3, §4). It is the GBI's `G_ENDDL` (`0xdf`)
/// masked to its command bits; `ReferenceBackend` reads it as the stream
/// terminator (`gbi/stream.rs`'s `G_ENDDL => break`).
///
/// Widened as **one id, not the block**. `0x10..=0x23` stays rejected around
/// it precisely so the mis-synchronization detector this comment describes
/// survives: a scan that has lost the command boundary still lands on a
/// rejected id with overwhelming probability, and
/// `undocumented_opcodes_are_still_rejected` pins the remaining region.
///
/// Width 8 is the wire fact, not a convenience: this is a one-command-word
/// id in the same No Operation block as `0x00..=0x07`. Nonclaim: assigning a
/// width says only how far to advance. It grants `0x1f` no terminator
/// semantic -- `WgpuBackend`'s decoder is length-delimited
/// (`raw_dpc/mod.rs`'s `while offset < stream.bytes.len()`), so it must not
/// choke on a terminator, and must not invent one either.
const RDP_STREAM_TERMINATOR_NOOP: u8 = 0x1f;
/// RDP-native command ids (bits 61:56), NOT the `0xc0`-based GBI spellings.
/// The GBI names differ by exactly that prefix: `G_TEXRECT` is `0xe4` and is
/// this `0x24` with bits 63:62 set. Widths are matched against the masked
/// command so every spelling resolves to the same entry.
const RDP_TEXRECT: u8 = 0x24;
const RDP_TEXRECTFLIP: u8 = 0x25;
const RDP_SYNC_LOAD: u8 = 0x26;
const RDP_SYNC_FULL: u8 = 0x29;

/// The outcome of scanning a raw-RDP command range.
///
/// A range can end in the middle of a command whose opcode width is known.
/// On hardware that is not an error: the DPC accepts END extensions in 8-byte
/// increments, so a 16-byte command straddles two END writes and CURRENT
/// simply stalls at that command's start until a later END exposes the rest.
/// `coalesce_dp_submissions` (`fn64-abi`) already documents and handles this
/// for RSP-produced streams; raw CPU MMIO ingress needs the same distinction,
/// and it could not have it while a truncated tail returned the same
/// `RenderError` as a genuinely malformed opcode.
///
/// **`Err(RenderError)` remains the third outcome and keeps its meaning.**
/// Only a KNOWN-width command overrunning the range end becomes
/// `Incomplete`; an unknown opcode, a misaligned or empty range, and an
/// address-space overflow all still reject loudly. Turning real garbage into
/// a silent stall is the one failure this type must not enable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawRdpScan<T, P> {
    /// Every command in the range was whole.
    Complete(T),
    /// The range ends inside a command. `complete_prefix` is the result
    /// accumulated from the whole commands before it.
    Incomplete {
        complete_prefix: T,
        command_start: P,
        bytes_required: u32,
        bytes_available: u32,
    },
}

impl<T, P> RawRdpScan<T, P> {
    /// The `Complete` payload, or `None` when the scan stalled.
    pub fn complete(self) -> Option<T> {
        match self {
            Self::Complete(value) => Some(value),
            Self::Incomplete { .. } => None,
        }
    }

    /// True when the range ends inside a known-width command.
    pub const fn is_incomplete(&self) -> bool {
        matches!(self, Self::Incomplete { .. })
    }
}

/// Inspect one exact raw DPC range for the command that generates the public
/// DP completion interrupt.
///
/// `rdram` is fn64's native-word storage representation; `start` and `end`
/// are logical guest byte offsets. The inspector uses [`fn64_runtime::RdramView`]
/// rather than treating storage bytes as a flat big-endian stream.
pub fn inspect_raw_rdp_full_sync(
    rdram: &[u8],
    start: u32,
    end: u32,
) -> Result<RawRdpScan<DpFullSyncStatus, u32>, RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "rdp-full-sync-inspection",
        reason,
    };
    if start >= end || !start.is_multiple_of(8) || !end.is_multiple_of(8) {
        return Err(reject(format!(
            "raw RDP range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
        )));
    }
    if end as usize > rdram.len() {
        return Err(reject(format!(
            "raw RDP range end {end:#010x} exceeds RDRAM length {:#x}",
            rdram.len()
        )));
    }

    let view = fn64_runtime::RdramView::from_storage(rdram);
    let mut reached = false;
    let mut pc = start;
    while pc < end {
        let wire_opcode = (view.read_u32(fn64_runtime::RdramAddr::from_offset(pc)) >> 24) as u8;
        // Bits 63:62 are don't-care; the command is the low six bits. Masking
        // here is what makes completion detection spelling-independent -- a
        // Sync Full emitted as 0x29, 0x69, 0xa9 or 0xe9 is the same command
        // and must all be seen, or the DP completion interrupt is missed.
        let opcode = wire_opcode & 0x3f;
        reached |= opcode == RDP_SYNC_FULL;
        let width = raw_rdp_command_width(opcode).ok_or_else(|| {
            reject(format!(
                "raw RDP opcode {opcode:#04x} (wire byte {wire_opcode:#04x}) at {pc:#010x} \
                 has no public command width"
            ))
        })?;
        pc = pc.checked_add(width).ok_or_else(|| {
            reject(format!(
                "raw RDP command at {pc:#010x} overflows address space"
            ))
        })?;
        if pc > end {
            // Known-width command overruns the range: STALL, not reject.
            // The prefix before it is whole and its completion result stands.
            let command_start = pc - width;
            return Ok(RawRdpScan::Incomplete {
                complete_prefix: if reached {
                    DpFullSyncStatus::Reached
                } else {
                    DpFullSyncStatus::NotReached
                },
                command_start,
                bytes_required: width,
                bytes_available: end - command_start,
            });
        }
    }

    Ok(RawRdpScan::Complete(if reached {
        DpFullSyncStatus::Reached
    } else {
        DpFullSyncStatus::NotReached
    }))
}

/// Count the `SYNC_FULL` sites in one owned raw-DPC command word image.
///
/// Unlike [`inspect_raw_rdp_full_sync`], which reads live RDRAM and answers a
/// yes/no question, this walks a caller-owned big-endian word image and
/// answers *how many* -- the number a producer needs in order to supply one
/// [`fn64_render_ir::FullSyncBoundary`] per decoded site, which
/// `fn64-render-ir`'s stream derivation requires exactly. It works for XBUS
/// captures too, whose words never live in RDRAM at all.
///
/// Uses the same structural stride walk and the same six-bit opcode masking
/// as the RDRAM inspector, for the same reason: a guessed stride
/// resynchronizes the scan onto payload bytes and lets a triangle
/// coefficient impersonate `G_RDPFULLSYNC`.
///
/// Nonclaim: a count is a count of decoded *sites*. It says nothing about
/// whether any DP interrupt was raised or observed for them.
pub fn count_raw_rdp_full_sync_sites(
    words: &[u32],
) -> Result<RawRdpScan<usize, usize>, RenderError> {
    let reject = |reason: String| RenderError::Backend {
        backend: "rdp-full-sync-site-count",
        reason,
    };
    let byte_len = words.len() * size_of::<u32>();
    let mut sites = 0_usize;
    let mut offset = 0_usize;
    while offset < byte_len {
        let wire_opcode = (words[offset / size_of::<u32>()] >> 24) as u8;
        let opcode = wire_opcode & 0x3f;
        if opcode == RDP_SYNC_FULL {
            sites += 1;
        }
        let width = raw_rdp_command_width(opcode).ok_or_else(|| {
            reject(format!(
                "raw RDP opcode {opcode:#04x} (wire byte {wire_opcode:#04x}) at word offset \
                 {offset:#x} has no public command width"
            ))
        })? as usize;
        offset += width;
        if offset > byte_len {
            // Known-width command overruns the image: STALL, not reject.
            let command_start = offset - width;
            return Ok(RawRdpScan::Incomplete {
                complete_prefix: sites,
                command_start,
                bytes_required: width as u32,
                bytes_available: (byte_len - command_start) as u32,
            });
        }
    }
    Ok(RawRdpScan::Complete(sites))
}

/// Byte width of one public raw RDP command, or `None` when no public
/// documentation assigns a command -- and therefore a stride -- at that
/// opcode. `None` is a rejection, never a hint to advance by eight.
///
/// `opcode` is the whole top wire byte. The RDP's command field is only
/// **bits 61:56** -- the low six bits of that byte -- and the command tables
/// mark bits 63:62 as don't-care (`Set Color Image` is spelled
/// `command = 0x3f[5:0]`). The hardware masks to six bits, so all four
/// spellings of a command are the same command, and this function masks
/// first for exactly that reason.
pub fn raw_rdp_command_width(opcode: u8) -> Option<u32> {
    // Bits 63:62 carry no command information. Reading the full byte made the
    // accepted spelling depend on which prefix a microcode happened to emit:
    // the `0xc0` form of every state command was accepted while the `0x00`,
    // `0x40` and `0x80` forms of the same command were rejected as unknown.
    // WCW/nWo Revenge emits `Set Color Image` as `0x7f` (`0x40 | 0x3f`) rather
    // than the `0xff` the GBI macros produce, and was rejected mid-frame for
    // it after twenty-three tasks had already scanned clean.
    let command = opcode & 0x3f;
    Some(match command {
        // The low no-operation block: 0x00..=0x07 are all *No Operation*, one
        // 64-bit command word each. `G_NOOP` is its 0x00 member under the
        // public GBI name, which is why 0x00 was accepted long before its
        // seven siblings were.
        G_NOOP..=RDP_LOW_NOOP_END => 8,
        // Carved out of the otherwise-rejected `0x10..=0x23` block on
        // measured evidence; see `RDP_STREAM_TERMINATOR_NOOP`.
        RDP_STREAM_TERMINATOR_NOOP => 8,
        // The eight triangle layouts. The low three bits select shade (4),
        // texture (2) and Z (1), and each enabled group appends coefficient
        // words to the 32-byte edge base.
        0x08 => 32,
        0x09 => 48,
        0x0a => 96,
        0x0b => 112,
        0x0c => 96,
        0x0d => 112,
        0x0e => 160,
        0x0f => 176,
        // Texture Rectangle / Texture Rectangle Flip are two command words.
        RDP_TEXRECT | RDP_TEXRECTFLIP => 16,
        // Every remaining assigned command -- the syncs, the tile and image
        // setup, the colour and combine state -- is a single command word.
        RDP_SYNC_LOAD..=0x3f => 8,
        // 0x10..=0x23 are documented *No Operation* but are deliberately not
        // accepted; see `RDP_LOW_NOOP_END`.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_word(storage: &mut [u8], offset: u32, word: u32) {
        fn64_runtime::RdramViewMut::from_storage(storage)
            .write_u32(fn64_runtime::RdramAddr::from_offset(offset), word);
    }

    #[test]
    fn full_sync_is_found_only_at_command_boundaries() {
        let mut rdram = vec![0; 40];
        write_word(&mut rdram, 0, 0x0800_0000);
        write_word(&mut rdram, 8, 0xe900_0000);
        assert_eq!(
            inspect_raw_rdp_full_sync(&rdram, 0, 32).unwrap(),
            RawRdpScan::Complete(DpFullSyncStatus::NotReached)
        );
        write_word(&mut rdram, 32, 0xe900_0000);
        assert_eq!(
            inspect_raw_rdp_full_sync(&rdram, 0, 40).unwrap(),
            RawRdpScan::Complete(DpFullSyncStatus::Reached)
        );
    }

    #[test]
    fn public_variable_width_commands_match_table_11() {
        for (opcode, width) in [
            (0x08, 32),
            (0x09, 48),
            (0x0a, 96),
            (0x0b, 112),
            (0x0c, 96),
            (0x0d, 112),
            (0x0e, 160),
            (0x0f, 176),
            (0xe4, 16),
            (0xe5, 16),
        ] {
            assert_eq!(raw_rdp_command_width(opcode), Some(width));
        }
    }

    /// Every id in the low No Operation block is one 64-bit command word.
    /// Pinned individually rather than as a range so a future edit that drops
    /// one of them fails on the specific opcode.
    #[test]
    fn low_no_operation_block_is_one_command_word_each() {
        for opcode in [0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07] {
            assert_eq!(
                raw_rdp_command_width(opcode),
                Some(8),
                "RDP No Operation {opcode:#04x} must be one 64-bit command word"
            );
        }
    }

    /// The widened map must still be able to say no. `0x10..=0x23` less the
    /// single measured carve-out is the only unaccepted region left, and it
    /// must reject under every prefix -- masking must not become a way to
    /// launder an unknown command.
    ///
    /// The `0x1f` exception is skipped here rather than deleted from the
    /// loop's range: keeping the range whole and naming the one hole makes it
    /// obvious that exactly one id was widened, and a second carve-out would
    /// have to be added here explicitly rather than slipping in under a
    /// shortened bound.
    #[test]
    fn undocumented_opcodes_are_still_rejected() {
        for command in 0x10u8..=0x23 {
            if command == RDP_STREAM_TERMINATOR_NOOP {
                continue;
            }
            for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
                let opcode = prefix | command;
                assert_eq!(
                    raw_rdp_command_width(opcode),
                    None,
                    "opcode {opcode:#04x} has no public command width and must not be accepted"
                );
            }
        }
    }

    /// The carve-out is exactly one id wide, and it is the measured one.
    ///
    /// Two assertions, because "0x1f is accepted" alone would also pass if
    /// the whole block had been widened -- which is the change this test
    /// exists to prevent. Its immediate neighbours must still reject.
    #[test]
    fn the_stream_terminator_is_widened_without_widening_its_block() {
        for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
            assert_eq!(
                raw_rdp_command_width(prefix | RDP_STREAM_TERMINATOR_NOOP),
                Some(8),
                "WM2000's measured stream terminator must be accepted under every prefix"
            );
        }
        for neighbour in [0x1eu8, 0x20] {
            assert_eq!(
                raw_rdp_command_width(neighbour),
                None,
                "widening {neighbour:#04x} alongside it would destroy the mis-synchronization \
                 detector the carve-out was kept narrow to preserve"
            );
        }
    }

    /// A rejected opcode must surface as an error from the scanner itself, not
    /// merely as a `None` width -- this is the path that stops a route. The
    /// message reports both the masked command and the raw wire byte, because
    /// the two differing is exactly the confusion that cost a Revenge run.
    #[test]
    fn scanner_rejects_an_undocumented_opcode_loudly() {
        let mut rdram = vec![0; 16];
        write_word(&mut rdram, 0, 0x9000_0000);
        write_word(&mut rdram, 8, 0xe900_0000);
        let error = inspect_raw_rdp_full_sync(&rdram, 0, 16).unwrap_err();
        let RenderError::Backend { backend, reason } = error else {
            panic!("undocumented raw RDP opcode must reject as a backend error");
        };
        assert_eq!(backend, "rdp-full-sync-inspection");
        assert!(
            reason.contains("0x10")
                && reason.contains("0x90")
                && reason.contains("no public command width"),
            "rejection must name both the command and the wire byte: {reason}"
        );
    }

    /// Bits 63:62 are don't-care, so all four spellings of a command must
    /// resolve identically. Revenge emits `Set Color Image` as `0x7f` where
    /// the GBI macros emit `0xff`; reading the full byte accepted only the
    /// latter and aborted the run mid-frame on the former.
    #[test]
    fn command_width_ignores_the_two_dont_care_prefix_bits() {
        for command in 0x00u8..=0x3f {
            let widths: Vec<_> = [0x00u8, 0x40, 0x80, 0xc0]
                .into_iter()
                .map(|prefix| raw_rdp_command_width(prefix | command))
                .collect();
            assert!(
                widths.iter().all(|width| *width == widths[0]),
                "command {command:#04x} resolves differently per prefix: {widths:?}"
            );
        }
        assert_eq!(raw_rdp_command_width(0x7f), Some(8));
        assert_eq!(raw_rdp_command_width(0xff), Some(8));
    }

    /// Completion detection must be spelling-independent too. A Sync Full the
    /// scanner fails to recognize is a DP completion interrupt never raised.
    #[test]
    fn full_sync_is_recognized_under_every_prefix_spelling() {
        for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
            let mut rdram = vec![0; 8];
            write_word(&mut rdram, 0, u32::from(prefix | RDP_SYNC_FULL) << 24);
            assert_eq!(
                inspect_raw_rdp_full_sync(&rdram, 0, 8).unwrap(),
                RawRdpScan::Complete(DpFullSyncStatus::Reached),
                "Sync Full spelled {:#04x} must be recognized",
                prefix | RDP_SYNC_FULL
            );
        }
    }

    /// A no-operation command advances by exactly one word, so a `FullSync`
    /// in the following word is still found. This is the behaviour Revenge
    /// needs: its microcode emits `0x07` mid-stream.
    #[test]
    fn low_no_operation_commands_do_not_desynchronize_the_scan() {
        for opcode in [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07] {
            let mut rdram = vec![0; 16];
            write_word(&mut rdram, 0, u32::from(opcode) << 24);
            write_word(&mut rdram, 8, 0xe900_0000);
            assert_eq!(
                inspect_raw_rdp_full_sync(&rdram, 0, 16).unwrap(),
                RawRdpScan::Complete(DpFullSyncStatus::Reached),
                "No Operation {opcode:#04x} must advance exactly one word"
            );
        }
    }

    #[test]
    fn texture_rectangle_payload_cannot_impersonate_full_sync() {
        for opcode in [0xe4u8, 0xe5] {
            let mut rdram = vec![0; 24];
            write_word(&mut rdram, 0, u32::from(opcode) << 24);
            write_word(&mut rdram, 8, 0xe900_0000);
            assert_eq!(
                inspect_raw_rdp_full_sync(&rdram, 0, 24).unwrap(),
                RawRdpScan::Complete(DpFullSyncStatus::NotReached)
            );
        }
    }
}
