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
const G_TEXRECT: u8 = 0xe4;
const G_TEXRECTFLIP: u8 = 0xe5;
const G_RDPLOADSYNC: u8 = 0xe6;
const G_RDPFULLSYNC: u8 = 0xe9;

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
        let triangle_opcode = wire_opcode & 0x3f;
        let opcode = if matches!(triangle_opcode, 0x08..=0x0f) {
            triangle_opcode
        } else {
            wire_opcode
        };
        reached |= opcode == G_RDPFULLSYNC;
        let width = raw_rdp_command_width(opcode).ok_or_else(|| {
            reject(format!(
                "raw RDP opcode {opcode:#04x} at {pc:#010x} has no public command width"
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
pub fn raw_rdp_command_width(opcode: u8) -> Option<u32> {
    let triangle_opcode = opcode & 0x3f;
    if matches!(triangle_opcode, 0x08..=0x0f) {
        return Some(match triangle_opcode {
            0x08 => 32,
            0x09 => 48,
            0x0a => 96,
            0x0b => 112,
            0x0c => 96,
            0x0d => 112,
            0x0e => 160,
            0x0f => 176,
            _ => unreachable!(),
        });
    }
    Some(match opcode {
        // The low no-operation block. `0x00` is the public `G_NOOP`; `0x01`
        // through `0x07` are the remaining No Operation ids of the same block
        // and are one command word each, exactly like `0x00`. Accepted only in
        // the RDP-native spelling, the same spelling the `0x08..=0x0f`
        // triangles above arrive in. The `0xc0`-based GBI spelling of these
        // ids (`0xc1..=0xc7`) is NOT accepted: the triangle normalization
        // above masks only `0x08..=0x0f`, so `0xc7` reaches this match as
        // `0xc7` and falls through to `None`. No public microcode emits that
        // form, and inventing a width for it would be a guess.
        G_NOOP..=RDP_LOW_NOOP_END => 8,
        G_TEXRECT | G_TEXRECTFLIP => 16,
        G_RDPLOADSYNC..=0xff => 8,
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
        write_word(&mut rdram, 8, u32::from(G_RDPFULLSYNC) << 24);
        assert_eq!(
            inspect_raw_rdp_full_sync(&rdram, 0, 32).unwrap(),
            DpFullSyncStatus::NotReached
        );
        write_word(&mut rdram, 32, u32::from(G_RDPFULLSYNC) << 24);
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
            (G_TEXRECT, 16),
            (G_TEXRECTFLIP, 16),
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

    /// The widened map must still be able to say no. These ids sit between the
    /// triangle block and `G_TEXRECT` and carry no documented public command,
    /// so advancing past them would be a guess.
    #[test]
    fn undocumented_opcodes_are_still_rejected() {
        for opcode in [0x10u8, 0x40, 0x7f, 0x80, 0xc7, 0xe3] {
            assert_eq!(
                raw_rdp_command_width(opcode),
                None,
                "opcode {opcode:#04x} has no public command width and must not be accepted"
            );
        }
    }

    /// A rejected opcode must surface as an error from the scanner itself, not
    /// merely as a `None` width -- this is the path that stops a route.
    #[test]
    fn scanner_rejects_an_undocumented_opcode_loudly() {
        let mut rdram = vec![0; 16];
        write_word(&mut rdram, 0, 0x4000_0000);
        write_word(&mut rdram, 8, u32::from(G_RDPFULLSYNC) << 24);
        let error = inspect_raw_rdp_full_sync(&rdram, 0, 16).unwrap_err();
        let RenderError::Backend { backend, reason } = error else {
            panic!("undocumented raw RDP opcode must reject as a backend error");
        };
        assert_eq!(backend, "rdp-full-sync-inspection");
        assert!(
            reason.contains("0x40") && reason.contains("no public command width"),
            "rejection must name the offending opcode: {reason}"
        );
    }

    /// A no-operation command advances by exactly one word, so a `FullSync`
    /// in the following word is still found. This is the behaviour Revenge
    /// needs: its microcode emits `0x07` mid-stream.
    #[test]
    fn low_no_operation_commands_do_not_desynchronize_the_scan() {
        for opcode in [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07] {
            let mut rdram = vec![0; 16];
            write_word(&mut rdram, 0, u32::from(opcode) << 24);
            write_word(&mut rdram, 8, u32::from(G_RDPFULLSYNC) << 24);
            assert_eq!(
                inspect_raw_rdp_full_sync(&rdram, 0, 16).unwrap(),
                DpFullSyncStatus::Reached,
                "No Operation {opcode:#04x} must advance exactly one word"
            );
        }
    }

    #[test]
    fn texture_rectangle_payload_cannot_impersonate_full_sync() {
        for opcode in [G_TEXRECT, G_TEXRECTFLIP] {
            let mut rdram = vec![0; 24];
            write_word(&mut rdram, 0, u32::from(opcode) << 24);
            write_word(&mut rdram, 8, u32::from(G_RDPFULLSYNC) << 24);
            assert_eq!(
                inspect_raw_rdp_full_sync(&rdram, 0, 24).unwrap(),
                DpFullSyncStatus::NotReached
            );
        }
    }
}
