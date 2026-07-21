//! Backend-neutral inspection of the public RDP completion command.
//!
//! Widths follow SGI *RDP Command Summary* Table 11. Triangle coefficient
//! payloads are skipped structurally so an arbitrary coefficient byte cannot
//! impersonate a command opcode.

use crate::{DpFullSyncStatus, RenderError};

const G_NOOP: u8 = 0x00;
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

/// Byte width of one public raw RDP command, or `None` when Table 11 assigns
/// no command at that opcode.
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
        G_NOOP => 8,
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
