//! Zelda-format overlay relocation harvesting.
//!
//! OoT/MM overlay files end with a relocation section the engine itself
//! consumes at load time: `[text_size, data_size, rodata_size, bss_size,
//! reloc_count, relocs..., pad, section_size]`, where the file's final
//! word is the section's distance from the end of the file. Each reloc
//! word types one location: bits 30-31 select the section (1 .text,
//! 2 .data, 3 .rodata), bits 24-29 the relocation kind (2 R_MIPS_32,
//! 4 R_MIPS_26, 5 HI16, 6 LO16), low 24 bits the offset.
//!
//! This is machine-checked structure — the engine relocates through this
//! very table on every overlay load — so a typed `R_MIPS_26` word IS a
//! call/jump and a typed `R_MIPS_32` data word IS an absolute pointer,
//! with none of the guessing a blind opcode scan needs. The parser is
//! strict and total: any inconsistency returns `None` (most files are
//! not overlays), never a partial result.
//!
//! HI16/LO16 pairs are consumed with the engine's own pairing rule —
//! BY REGISTER, not by adjacency: each HI16 saves its immediate under
//! the lui's rt register, and each LO16 looks up its own rs register
//! (compilers interleave pairs as `lui A; lui B; addiu A; addiu B`, so
//! most-recent-HI16 pairing constructs garbage — measured as a split
//! root landing in BgHidanHamstep_Draw's delay slot on OoT). This
//! mirrors Overlay_Relocate's per-register tracking, yielding the
//! addresses actor code materializes as constants: `this->actionFunc =
//! Func` compiles to a typed lui/addiu pair. R_MIPS_26 `j` (not `jal`)
//! targets are skipped: a plain jump proves code, not a callable entry.
//!
//! Measured result (MM, 2026-07-20): all 612 proven overlays parse, and
//! every reloc-typed boot-window reference was already found by the
//! blind cross-bank jal scan and the lui/addiu harvest — 0 new roots.
//! The lane's value on MM is therefore *corroboration*: the engine's own
//! relocation tables machine-confirm those scans' completeness, which
//! upgrades "no references found" to "conclusively unreferenced" for the
//! remaining open boot functions. Typed references may still add recall
//! on games whose overlays are not also covered by proven-bank scans.

/// References harvested from one overlay's relocation section.
#[derive(Debug, Default, Clone)]
pub struct OverlayRelocRefs {
    /// Absolute targets of `jal` words typed `R_MIPS_26` in `.text` —
    /// machine-checked callable entries.
    pub jal_targets: Vec<u32>,
    /// Absolute values of words typed `R_MIPS_32` in `.data` — stored
    /// pointers; when they name text, they are callback fields (actor
    /// init/destroy/update/draw), i.e. callable entries.
    pub data_pointers: Vec<u32>,
    /// Words typed `R_MIPS_32` in `.rodata`, as `(file_offset, value)`
    /// sorted by offset. Zelda overlays load file-layout-identical, so a
    /// word's VA is `bank_va_start + file_offset`. Kept separate from
    /// `.data` deliberately: rodata pointer arrays are dominated by
    /// switch jump tables, whose entries are MID-FUNCTION case labels —
    /// seeding them as roots split 25 real functions across 10 MM actor
    /// overlays (measured 2026-07-20). As located, typed runs they are
    /// exactly the jump-table evidence the open-`jr` frontier needs:
    /// the typing bounds the table where value-set analysis cannot.
    pub rodata_words: Vec<(u32, u32)>,
    /// Addresses constructed by typed HI16/LO16 pairs (most-recent-HI16
    /// pairing, signed LO for addiu/loads, unsigned for ori) — the
    /// constants overlay code materializes, dominated by action-function
    /// assignments.
    pub hi_lo_pointers: Vec<u32>,
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}

/// Parse `bytes` as a whole Zelda-format overlay file. Returns `None`
/// unless every structural invariant holds.
pub fn parse_zelda_overlay(bytes: &[u8]) -> Option<OverlayRelocRefs> {
    let len = bytes.len();
    if len < 0x20 || !len.is_multiple_of(4) {
        return None;
    }
    let section_size = read_u32(bytes, len - 4)? as usize;
    // Header (5 words) + final size word must fit; section lives at the
    // file tail.
    if section_size < 0x18 || section_size > len || !section_size.is_multiple_of(4) {
        return None;
    }
    let header = len - section_size;
    let text = read_u32(bytes, header)? as usize;
    let data = read_u32(bytes, header + 4)? as usize;
    let rodata = read_u32(bytes, header + 8)? as usize;
    let _bss = read_u32(bytes, header + 12)?;
    let reloc_count = read_u32(bytes, header + 16)? as usize;
    if !text.is_multiple_of(4) || !data.is_multiple_of(4) || !rodata.is_multiple_of(4) {
        return None;
    }
    // Sections precede the reloc section exactly (padding only between
    // rodata end and header).
    let sections_end = text.checked_add(data)?.checked_add(rodata)?;
    if sections_end > header || header - sections_end >= 16 {
        return None;
    }
    // 5 header words + reloc words + final size word, padded to 16.
    if 0x14 + reloc_count * 4 + 4 > section_size {
        return None;
    }
    let mut refs = OverlayRelocRefs::default();
    let mut hi16_by_reg: [Option<u32>; 32] = [None; 32];
    for index in 0..reloc_count {
        let word = read_u32(bytes, header + 0x14 + index * 4)?;
        let section = word >> 30;
        let kind = (word >> 24) & 0x3f;
        let offset = (word & 0x00ff_ffff) as usize;
        let (base, size) = match section {
            1 => (0usize, text),
            2 => (text, data),
            3 => (text + data, rodata),
            _ => return None,
        };
        if !offset.is_multiple_of(4) || offset + 4 > size {
            return None;
        }
        let value = read_u32(bytes, base + offset)?;
        match kind {
            5 => {
                // HI16 must sit on a lui; save under its rt register.
                if value >> 26 != 0x0f {
                    return None;
                }
                hi16_by_reg[((value >> 16) & 0x1f) as usize] = Some(value & 0xffff);
            }
            6 => {
                // LO16 pairs with the HI16 that loaded its rs register.
                let hi = hi16_by_reg[((value >> 21) & 0x1f) as usize]?;
                let lo = value & 0xffff;
                let target = if value >> 26 == 0x0d {
                    (hi << 16) | lo // ori: unsigned
                } else {
                    ((hi << 16) as i64 + (lo as i16 as i64)) as u32
                };
                refs.hi_lo_pointers.push(target);
            }
            4 => {
                // R_MIPS_26 must sit on a j/jal instruction.
                match value >> 26 {
                    0x03 => refs
                        .jal_targets
                        .push(0x8000_0000 | ((value & 0x03ff_ffff) << 2)),
                    0x02 => {}
                    _ => return None,
                }
            }
            2 if section == 2 => refs.data_pointers.push(value),
            2 if section == 3 => refs.rodata_words.push(((base + offset) as u32, value)),
            2 => {}
            _ => return None,
        }
    }
    refs.jal_targets.sort_unstable();
    refs.jal_targets.dedup();
    refs.data_pointers.sort_unstable();
    refs.data_pointers.dedup();
    refs.rodata_words.sort_unstable();
    refs.rodata_words.dedup();
    refs.hi_lo_pointers.sort_unstable();
    refs.hi_lo_pointers.dedup();
    Some(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// text: [jal 0x80081730, nop, jr ra, nop], data: [ptr 0x80086588],
    /// rodata: empty, relocs: R_MIPS_26 @text+0, R_MIPS_32 @data+0.
    fn overlay() -> Vec<u8> {
        let mut bytes = Vec::new();
        word(&mut bytes, 0x0c000000 | ((0x80081730 & 0x03ff_ffff) >> 2)); // jal
        word(&mut bytes, 0);
        word(&mut bytes, 0x03e00008);
        word(&mut bytes, 0);
        word(&mut bytes, 0x80086588); // .data pointer
                                      // reloc section: 5 header words, 2 relocs, pad, size word
        let header_start = bytes.len();
        word(&mut bytes, 16); // text
        word(&mut bytes, 4); // data
        word(&mut bytes, 0); // rodata
        word(&mut bytes, 0); // bss
        word(&mut bytes, 2); // count
        word(&mut bytes, (1 << 30) | (4 << 24)); // R_MIPS_26 .text+0
        word(&mut bytes, (2 << 30) | (2 << 24)); // R_MIPS_32 .data+0
        word(&mut bytes, 0); // pad
        let section_size = bytes.len() - header_start + 4;
        word(&mut bytes, section_size as u32);
        bytes
    }

    #[test]
    fn parses_typed_references() {
        let refs = parse_zelda_overlay(&overlay()).expect("valid overlay");
        assert_eq!(refs.jal_targets, vec![0x80081730]);
        assert_eq!(refs.data_pointers, vec![0x80086588]);
    }

    #[test]
    fn rejects_non_overlay_bytes() {
        assert!(parse_zelda_overlay(&[0u8; 64]).is_none());
        let mut broken = overlay();
        let len = broken.len();
        broken[len - 1] = 0xff; // absurd section size
        assert!(parse_zelda_overlay(&broken).is_none());
    }

    #[test]
    fn rejects_reloc26_on_non_jump_word() {
        let mut bytes = overlay();
        // Overwrite the jal with an addiu; the typed reloc now lies.
        bytes[0] = 0x24;
        assert!(parse_zelda_overlay(&bytes).is_none());
    }
}
