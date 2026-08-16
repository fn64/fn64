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
/// RDP-native command ids (bits 61:56), NOT the `0xc0`-based GBI spellings.
/// The GBI names differ by exactly that prefix: `G_TEXRECT` is `0xe4` and is
/// this `0x24` with bits 63:62 set. Widths are matched against the masked
/// command so every spelling resolves to the same entry.
const RDP_TEXRECT: u8 = 0x24;
const RDP_TEXRECTFLIP: u8 = 0x25;
const RDP_SYNC_LOAD: u8 = 0x26;
const RDP_SYNC_FULL: u8 = 0x29;

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
) -> Result<DpFullSyncStatus, RenderError> {
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
            return Err(reject(format!(
                "raw RDP command at {:#010x} is truncated by range end {end:#010x}",
                pc - width
            )));
        }
    }

    Ok(if reached {
        DpFullSyncStatus::Reached
    } else {
        DpFullSyncStatus::NotReached
    })
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
            DpFullSyncStatus::NotReached
        );
        write_word(&mut rdram, 32, 0xe900_0000);
        assert_eq!(
            inspect_raw_rdp_full_sync(&rdram, 0, 40).unwrap(),
            DpFullSyncStatus::Reached
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

    /// The widened map must still be able to say no. `0x10..=0x23` is the
    /// only unaccepted region left, and it must reject under every prefix --
    /// masking must not become a way to launder an unknown command.
    #[test]
    fn undocumented_opcodes_are_still_rejected() {
        for command in 0x10u8..=0x23 {
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
                DpFullSyncStatus::Reached,
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
                DpFullSyncStatus::Reached,
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
                DpFullSyncStatus::NotReached
            );
        }
    }
}
