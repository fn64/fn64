//! ROM-bound geometry for the byte-verified AKI/NW4E overlay table.
//!
//! The shape is data, not a scanner heuristic: callers must still bind it to
//! the normalized ROM identity through the evidence/manifest path.

use crate::banks::DescriptorTableShape;

/// NW4E's five fixed overlay records at ROM `0x539a0`.
pub const NW4E_DESCRIPTOR_TABLE: DescriptorTableShape = DescriptorTableShape {
    table_rom_offset: 0x0539a0,
    record_count: 5,
    record_stride: 0x24,
    field_rom_start: 0x00,
    field_rom_end: 0x04,
    field_vram_dest: 0x08,
};

pub fn nw4e_bank_name(index: u32) -> String {
    format!("R{}", index + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nw4eBankGeometry {
    pub bank: &'static str,
    pub slot: char,
    pub rom_start: u32,
    pub rom_end: u32,
    pub va_start: u32,
    pub va_end: u32,
}

/// Byte-verified load ranges from the NW4E overlay manifest.  This is the
/// canonical target set for selector/data-flow analysis; it does not encode
/// when a bank is selected.
pub const NW4E_BANKS: [Nw4eBankGeometry; 5] = [
    Nw4eBankGeometry {
        bank: "R1",
        slot: 'A',
        rom_start: 0x057310,
        rom_end: 0x081210,
        va_start: 0x800d9960,
        va_end: 0x80103860,
    },
    Nw4eBankGeometry {
        bank: "R2",
        slot: 'B',
        rom_start: 0x081210,
        rom_end: 0x0ae390,
        va_start: 0x80106760,
        va_end: 0x801338e0,
    },
    Nw4eBankGeometry {
        bank: "R3",
        slot: 'B',
        rom_start: 0x0ae390,
        rom_end: 0x0fd250,
        va_start: 0x80106760,
        va_end: 0x80155620,
    },
    Nw4eBankGeometry {
        bank: "R4",
        slot: 'A',
        rom_start: 0x144be0,
        rom_end: 0x1bd150,
        va_start: 0x800d9960,
        va_end: 0x80151ed0,
    },
    Nw4eBankGeometry {
        bank: "R5",
        slot: 'B',
        rom_start: 0x0fd250,
        rom_end: 0x144be0,
        va_start: 0x80106760,
        va_end: 0x8014e0f0,
    },
];

/// Static selector evidence recovered from the NW4E loader dispatcher at ROM
/// `0x27488`, VA `0x80026888`. The flag word is `0x800a10b0`; the masks are
/// branch predicates, not a claim about which game state sets them.
///
/// # VA correction (2026-07-18)
///
/// An earlier revision recorded the dispatcher at VA `0x80027488`, assuming
/// the resident mapping `VA = ROM + 0x8000_0000`. That contradicts the
/// byte-verified boot facts: the ROM header entry point is `0x80000400` and
/// the IPL3 copy source is ROM `0x1000` (the entry stub bytes live there),
/// so the resident delta is `0x7fff_f400` and ROM `0x27488` executes at
/// `0x80026888`. The disambiguating evidence is mapping-independent: every
/// absolute `jal` inside the dispatcher body lands on a classic
/// `addiu $sp,$sp,-N` prologue only under the `0x7fff_f400` delta (checked
/// for all twelve `jal` sites in the dispatcher region). All PCs below carry
/// the corrected values; the masks and structure were unaffected.
///
/// # Reachability
///
/// No `j`/`jal`/branch in any NW4E bank targets the dispatcher. Its entry is
/// data-derived: the wrapper at `thread_create_wrapper_va` materializes the
/// dispatcher address (`lui`/`addiu` ending at `entry_materialize_pc`) into
/// `$a2` and passes it to the thread-create/start pair at
/// `thread_create_call_va`/`thread_start_call_va`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nw4eSelectorEvidence {
    pub dispatcher_va: u32,
    pub flag_va: u32,
    /// Companion mode byte the dispatcher writes on each branch path
    /// (observed constants: 0 at init/R3, 3 before R2, 2 before R5, 1 after
    /// the loop).
    pub mode_byte_va: u32,
    pub loop_mask: u32,
    pub r2_skip_mask: u32,
    pub r3_skip_mask: u32,
    pub r5_take_mask: u32,
    pub r4_loaded_after_loop: bool,
    /// PC of `sw $zero, %lo(flag)(at)`: the dispatcher zero-initializes the
    /// flag itself before the loop, so every later value is a runtime store.
    pub init_store_pc: u32,
    /// Loop head: the R1 record load that starts every iteration.
    pub loop_head_pc: u32,
    /// Each `lui` beginning a `lw flag / andi mask / branch` test.
    pub r2_test_pc: u32,
    pub r3_test_pc: u32,
    pub r5_test_pc: u32,
    pub loop_test_pc: u32,
    /// First-phase record loader called with nine record fields.
    pub record_loader_va: u32,
    /// Second-phase per-record callee invoked after the loader on the
    /// R2/R3/R5 paths.
    pub record_phase2_va: u32,
    /// The thread-create wrapper that takes the dispatcher's address.
    pub thread_create_wrapper_va: u32,
    /// PC of the `addiu` completing the dispatcher-address materialization.
    pub entry_materialize_pc: u32,
    pub thread_create_call_va: u32,
    pub thread_start_call_va: u32,
}

pub const NW4E_SELECTOR: Nw4eSelectorEvidence = Nw4eSelectorEvidence {
    dispatcher_va: 0x80026888,
    flag_va: 0x800a10b0,
    mode_byte_va: 0x80097fd8,
    loop_mask: 0x2,
    r2_skip_mask: 0x1,
    r3_skip_mask: 0x8,
    r5_take_mask: 0x40,
    r4_loaded_after_loop: true,
    init_store_pc: 0x800268f0,
    loop_head_pc: 0x800268fc,
    r2_test_pc: 0x80026938,
    r3_test_pc: 0x800269d0,
    r5_test_pc: 0x80026a64,
    loop_test_pc: 0x80026afc,
    record_loader_va: 0x8000073c,
    record_phase2_va: 0x800007ac,
    thread_create_wrapper_va: 0x80026830,
    entry_materialize_pc: 0x80026860,
    thread_create_call_va: 0x80037520,
    thread_start_call_va: 0x800376e0,
};

pub fn nw4e_bank_for_rom_start(rom_start: u32) -> Option<Nw4eBankGeometry> {
    NW4E_BANKS
        .iter()
        .copied()
        .find(|bank| bank.rom_start == rom_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nw4e_shape_is_fixed_and_named_deterministically() {
        assert_eq!(NW4E_DESCRIPTOR_TABLE.record_count, 5);
        assert_eq!(NW4E_DESCRIPTOR_TABLE.record_stride, 0x24);
        assert_eq!(nw4e_bank_name(0), "R1");
        assert_eq!(nw4e_bank_name(4), "R5");
        assert_eq!(nw4e_bank_for_rom_start(0x057310).unwrap().slot, 'A');
        assert_eq!(nw4e_bank_for_rom_start(0x0fd250).unwrap().bank, "R5");
        assert!(nw4e_bank_for_rom_start(0x123456).is_none());
        assert_eq!(NW4E_SELECTOR.dispatcher_va, 0x80026888);
        assert_eq!(NW4E_SELECTOR.flag_va, 0x800a10b0);
        assert_eq!(NW4E_SELECTOR.loop_mask, 0x2);
        // Resident delta is 0x7fff_f400 (header entry 0x80000400 <- ROM
        // 0x1000), so the dispatcher VA and its ROM offset differ by it.
        assert_eq!(NW4E_SELECTOR.dispatcher_va - 0x7fff_f400, 0x27488);
    }
}
