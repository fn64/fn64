//! Legacy generated-C byte mirror for the `0xA4xxxxxx`/`0xA8xxxxxx` KSEG1
//! range: AI, VI, PI, SI, DP, MI.
//!
//! This module is not production device authority. Timed AI and transactional
//! DPC state live only in [`crate::device::DeviceFabric`]; typed/interpreted
//! and generated-C proxy accesses must reach that fabric before this fallback.
//! `AiRegs` and `DpRegs` remain solely to materialize safe compatibility bytes
//! for native generated C whose pointer loads cannot be intercepted. Their
//! state must never start a DMA, select a renderer range, raise MI, or enter a
//! release snapshot.
//!
//! ## Why this exists (real crash this closes)
//!
//! `Rdram` (`rdram.rs`) is a flat `DEFAULT_RDRAM_SIZE` (8 MB) byte buffer.
//! Real N64 hardware maps `0xA4xxxxxx` to a *disjoint* physical address
//! space -- memory-mapped hardware registers, not RDRAM at all. Generated
//! `RecompiledFuncs/*.c` code does not always go through an `osXxx_recomp`
//! shim to reach these registers: `docs/BOOT-NOTES-WM2000.md`'s rung-3+
//! frontier hit a **raw MIPS load**, `lui $v0,0xA450; ori $v0,$v0,0xC; lw
//! $v0,0($v0)` (an inlined `MEM_W(0xA450000C, 0)` reading `AI_STATUS`
//! directly, with no shim call at all -- confirmed via LLDB backtrace,
//! `func_8002B890`). `MEM_W`'s translation (`rdram.rs`'s
//! `RdramAddr::from_gpr`: `reg - 0xFFFFFFFF80000000`) on that address
//! computes rdram-relative offset `0x2450_000C` -- about 4x past an 8 MB
//! buffer's end, an out-of-bounds host read/panic, confirmed by that same
//! session's LLDB `EXC_BAD_ACCESS`.
//!
//! Since `MEM_W` is a plain pointer dereference baked directly into
//! generated C (no host-side interception point exists there -- see
//! `rdram.rs`'s own module doc), the only way to make a raw load/store at
//! this address range both memory-safe AND observably correct is for the
//! **backing buffer itself to be large enough to cover this offset window**,
//! with the real bytes living there kept in sync with this module's
//! register model. `Rdram::new`/any harness's own rdram allocation is
//! responsible for sizing the buffer to at least
//! `RDRAM_MMIO_WINDOW_END` and calling `MmioSpace::sync_into_rdram`/
//! `sync_from_rdram` at the right points (see those methods' doc comments)
//! -- this module owns the register semantics; it does not itself own the
//! buffer.
//!
//! ## Provenance
//!
//! Register layout/bit meanings are the publicly documented N64 hardware
//! memory map (`n64dev`/homebrew community references: AI/VI/PI/SI/SP/DP/MI
//! base addresses and register offsets are widely published hardware
//! documentation, not GPL runtime internals -- no ultramodern/librecomp
//! source was read for this module). Values returned favor "DMA/task
//! proceeds immediately" (not-busy, not-full) since this crate has no real
//! async DMA timing model (`docs/DESIGN.md`'s "no wall-clock in core" /
//! host-driven-completion design already established for PI DMA in
//! `rom.rs` and RSP tasks in `rsp.rs`) -- a register model that reported
//! "busy forever" would deadlock any real guest polling loop, which is a
//! worse failure than an honestly-approximate "always ready" value.
//!
//! ## Scope
//!
//! This is a **register-read/write model**, not a hardware simulation: it
//! tracks the handful of fields real code actually polls or sets (status
//! bits, current buffer pointers, DMA length) as plain host state, with no
//! timing/interrupt-latency modeling. Every field defaults to the value
//! real firmware observes when the corresponding unit is idle/uninitialized
//! at boot.
//!
//! SP memory, DMA, status, semaphore, and PC are deliberately absent here:
//! [`crate::device::DeviceFabric`] is their one authoritative model and raw
//! typed/generated-C accesses are intercepted there before this legacy mirror.
//! Keeping a second SP register file here previously mislabeled `SP_WR_LEN` at
//! `0xA404_000c` as the PC (the real CPU-visible PC is `0xA408_0000`) and could
//! not express timed DMA. An access that bypasses the device fabric must trap
//! rather than observe that stale shadow.
//!
//! Address decoding (which rdram-relative offset -> which unit -> which
//! register) lives in `MmioSpace::read_w`/`write_w`, the ONE place that
//! maps an already-`RdramAddr`-translated offset to a specific register --
//! deliberately centralized so a future new register doesn't need a second,
//! ad hoc decode site. All offsets in this module are in the SAME space
//! `RdramAddr::offset()` returns (post KSEG0/1-base subtraction), NOT raw
//! `0xA4xxxxxx` addresses -- see `is_mmio_offset`'s doc comment for the
//! exact math, which is what a real caller (`Rdram`, or a shim holding a
//! `RdramAddr`) already has in hand.

/// The `RdramAddr`-space offset (i.e. what `RdramAddr::from_gpr` returns,
/// `real_address - 0xFFFFFFFF80000000`) where the hardware-register window
/// begins. Real N64 KSEG1 register base `0xA400_0000`, minus the same
/// `0xFFFFFFFF80000000` `MEM_*` subtracts, lands at `0x2400_0000` -- this is
/// the offset `Rdram`/a raw `rdram` pointer must have real bytes backing
/// starting from, and the base every `base::*` constant below is relative
/// to.
pub const RDRAM_MMIO_WINDOW_START: u32 = 0x2400_0000;

/// One-past-the-end of the modeled hardware-register window, `RdramAddr`
/// space. This is the backing-allocation extent for raw generated KSEG1
/// accesses, including cartridge-domain addresses that are not decoded as RCP
/// registers. The narrower [`RDRAM_RCP_MMIO_END`] is the decoder boundary.
pub const RDRAM_MMIO_WINDOW_END: u32 = 0x2900_0000;

/// Exclusive end of the actual RCP register decode. SI starts at
/// `0xA480_0000` and is the final block before `0xA490_0000`; translated into
/// `RdramAddr` space, that boundary is `0x2490_0000`. Cartridge-domain KSEG1
/// addresses such as `0xA600_1010` remain ordinary backed memory.
pub const RDRAM_RCP_MMIO_END: u32 = 0x2490_0000;

/// Base offsets of each hardware unit's register block, relative to
/// `RDRAM_MMIO_WINDOW_START` (i.e. already in `RdramAddr`-offset space, NOT
/// raw `0xA4xxxxxx` addresses). Per the public N64 hardware memory map:
/// real KSEG1 bases are `SP=0xA404_0000`, `DP_CMD=0xA410_0000`,
/// `MI=0xA430_0000`, `VI=0xA440_0000`, `AI=0xA450_0000`, `PI=0xA460_0000`,
/// `SI=0xA480_0000` -- each below is that base minus
/// `RDRAM_MMIO_WINDOW_START`'s corresponding `0xA400_0000`.
mod base {
    pub const DP_CMD: u32 = 0x0010_0000;
    pub const MI: u32 = 0x0030_0000;
    pub const VI: u32 = 0x0040_0000;
    pub const AI: u32 = 0x0050_0000;
    pub const PI: u32 = 0x0060_0000;
    pub const SI: u32 = 0x0080_0000;
}

/// Compatibility-only AI byte image. Real register offsets (from
/// `base::AI`): `DRAM_ADDR` @0x00, `LEN` @0x04, `CONTROL` @0x08, `STATUS`
/// @0x0C (write-any-value clears the AI interrupt; read returns status
/// bits), `DACRATE` @0x10, `BITRATE` @0x14.
#[derive(Debug, Default)]
pub struct AiRegs {
    pub dram_addr: u32,
    pub len: u32,
    pub control: u32,
    pub dacrate: u32,
    pub bitrate: u32,
    /// Whether a DMA is currently modeled as "in flight." This crate has no
    /// async audio-DMA timing (see module doc), so `start_dma`/`set_len`
    /// mark this `true` only until the next status read, at which point it
    /// is reported once then cleared -- long enough for a real polling loop
    /// (`while (osAiGetStatus() & AI_STATUS_BUSY) {}`) to observe "was busy,
    /// now done" without ever `unimplemented!()`-panicking or spinning
    /// forever on an eternally-busy fake status.
    dma_pending: bool,
}

/// `AI_STATUS` register bits, per the public libultra manual / hardware
/// docs (`AI_STATUS_ENABLED = 1<<25`, `AI_STATUS_BUSY = 1<<30`,
/// `AI_STATUS_FULL = 1<<31`).
pub const AI_STATUS_ENABLED: u32 = 1 << 25;
pub const AI_STATUS_BUSY: u32 = 1 << 30;
pub const AI_STATUS_FULL: u32 = 1 << 31;

impl AiRegs {
    /// Mirror `osAiSetNextBuffer` bytes for generated-C loads. This does not
    /// start a device DMA; production work enters `DeviceFabric`. Latch a DMA source/length and mark it
    /// pending. Faithful "DMA proceeds" behavior (module doc): the very
    /// next status read reports not-busy/not-full so a guest audio-manager
    /// loop that submits a buffer then immediately polls status is never
    /// wedged.
    pub fn set_next_buffer(&mut self, addr: u32, len: u32) {
        self.dram_addr = addr;
        self.len = len;
        self.dma_pending = true;
    }

    /// `osAiGetStatus() -> u32`. One-shot: reports the pending DMA as busy
    /// exactly once (so a caller that checks "did my last submit start"
    /// sees a true bit at least once), then clears it -- see `dma_pending`'s
    /// doc comment.
    pub fn status(&mut self) -> u32 {
        let enabled = if self.control & 1 != 0 {
            AI_STATUS_ENABLED
        } else {
            0
        };
        if self.dma_pending {
            self.dma_pending = false;
            enabled | AI_STATUS_BUSY
        } else {
            enabled
        }
    }

    /// `osAiGetLength() -> u32`: length remaining in the current/last DMA.
    /// Real hardware counts this down as samples drain; with no async DMA
    /// timing this crate reports the full latched length until overwritten
    /// by the next `set_next_buffer` (never fabricating a fake mid-drain
    /// value with no evidence behind it).
    pub fn length(&self) -> u32 {
        self.len
    }
}

/// VI (Video Interface) register block, the raw-register counterpart to
/// `vi.rs`'s `ViState` (which models the libultra `osVi*` shim-level API).
/// Only the fields a polling loop plausibly reads are modeled: `CURRENT`
/// (the line the video beam is on) and `STATUS`.
#[derive(Debug, Default)]
pub struct ViRegs {
    pub current_line: u32,
    pub status: u32,
}

/// PI (Peripheral/Parallel Interface, cartridge + ROM DMA) register block.
/// `STATUS` bits: `PI_STATUS_DMA_BUSY = 1`, `PI_STATUS_IO_BUSY = 2`,
/// `PI_STATUS_ERROR = 4` (public hardware docs). This crate's real PI DMA
/// (`rom.rs::PiDma`) completes synchronously, so `status` here always
/// reports idle (0) -- matching "DMA proceeds" the same way `AiRegs` does.
#[derive(Debug, Default)]
pub struct PiRegs {
    pub dram_addr: u32,
    pub cart_addr: u32,
    pub status: u32,
}

/// SI (Serial Interface, PIF/controller) register block. `STATUS` bit
/// `SI_STATUS_DMA_BUSY = 1`, `SI_STATUS_IO_BUSY = 2`. This crate's SI model
/// (`si.rs::PifModel`) is synchronous, so status always reports idle.
#[derive(Debug, Default)]
pub struct SiRegs {
    pub status: u32,
}

fn apply_clear_set_pair(
    state: &mut u32,
    command: u32,
    clear_command: u32,
    set_command: u32,
    state_bit: u32,
) {
    if command & (1 << clear_command) != 0 {
        *state &= !(1 << state_bit);
    }
    if command & (1 << set_command) != 0 {
        *state |= 1 << state_bit;
    }
}

/// Compatibility-only DP byte image. `STATUS` bits:
/// `DP_STATUS_XBUS_DMA = 1`, `DP_STATUS_FREEZE = 2`, `DP_STATUS_FLUSH = 4`,
/// `DP_STATUS_START_GCLK = 0x08`, `DP_STATUS_TMEM_BUSY = 0x10`,
/// `DP_STATUS_PIPE_BUSY = 0x20`, `DP_STATUS_CMD_BUSY = 0x40`,
/// `DP_STATUS_CBUF_READY = 0x80`, `DP_STATUS_DMA_BUSY = 0x100`,
/// `DP_STATUS_END_VALID = 0x200`, `DP_STATUS_START_VALID = 0x400`. Idle (0)
/// by default -- no real RDP command execution happens in this crate
/// (`fn64-render`/`fn64-render-rt64` own the actual GBI interpretation, per
/// `docs/DECOUPLING.md`).
#[derive(Debug, Default)]
pub struct DpRegs {
    pub start: u32,
    pub end: u32,
    pub current: u32,
    pub status: u32,
}

pub const DP_STATUS_XBUS_DMA: u32 = 1;
pub const DP_STATUS_FREEZE: u32 = 1 << 1;
pub const DP_STATUS_FLUSH: u32 = 1 << 2;
pub const DP_STATUS_START_VALID: u32 = 1 << 10;

impl DpRegs {
    /// Apply the DP status register's paired clear/set command bits. Counter
    /// reset commands have no observable counter state in this model.
    pub fn apply_status_command(&mut self, command: u32) {
        if command & 0x01 != 0 {
            self.status &= !DP_STATUS_XBUS_DMA;
        }
        if command & 0x02 != 0 {
            self.status |= DP_STATUS_XBUS_DMA;
        }
        if command & 0x04 != 0 {
            self.status &= !DP_STATUS_FREEZE;
        }
        if command & 0x08 != 0 {
            self.status |= DP_STATUS_FREEZE;
        }
        if command & 0x10 != 0 {
            self.status &= !DP_STATUS_FLUSH;
        }
        if command & 0x20 != 0 {
            self.status |= DP_STATUS_FLUSH;
        }
    }

    /// Mirror one consumed DRAM-backed RDP command range. The current HLE renderer
    /// consumes graphics work synchronously, so the observable current
    /// pointer reaches `end` and no busy/valid bit remains set on return.
    pub fn set_next_buffer(&mut self, start: u32, end: u32) -> bool {
        if self.status & DP_STATUS_START_VALID != 0 {
            return false;
        }
        self.start = start;
        self.end = end;
        self.current = end;
        true
    }
}

/// MI (MIPS Interface) register block: the top-level interrupt-mask/
/// interrupt-pending registers every other unit's interrupt line funnels
/// through. `MI_INTR_MASK`/`MI_INTR` bit layout (public hardware docs):
/// bit0 SP, bit1 SI, bit2 AI, bit3 VI, bit4 PI, bit5 DP.
#[derive(Debug, Default)]
pub struct MiRegs {
    pub intr_mask: u32,
    pub intr: u32,
    pub mode: u32,
}

impl MiRegs {
    /// Apply the public MI mode command register. The write bits are commands,
    /// while the readback mode bits occupy different positions. DP interrupt
    /// acknowledgement belongs to the integrated interrupt fabric, so bit 11
    /// is intentionally not represented in this register-only view.
    pub fn apply_mode_command(&mut self, command: u32) {
        apply_clear_set_pair(&mut self.mode, command, 7, 8, 7); // INIT
        apply_clear_set_pair(&mut self.mode, command, 9, 10, 8); // EBUS
        apply_clear_set_pair(&mut self.mode, command, 12, 13, 9); // RDRAM
    }

    /// Apply the six paired clear/set commands in `MI_INTR_MASK_REG`.
    pub fn apply_mask_command(&mut self, command: u32) {
        for source in 0..6 {
            apply_clear_set_pair(
                &mut self.intr_mask,
                command,
                source * 2,
                source * 2 + 1,
                source,
            );
        }
    }
}

/// The legacy hardware-register byte-mirror space: one instance per running game,
/// alongside (not inside) `Rdram` -- see `rdram.rs`'s doc comment on why
/// `Rdram` itself stays a plain flat buffer rather than growing a special
/// case for this range (this module is the dedicated seam instead, and
/// `sync_into_rdram`/`sync_from_rdram` are the bridge between the two).
#[derive(Debug, Default)]
pub struct MmioSpace {
    pub ai: AiRegs,
    pub vi: ViRegs,
    pub pi: PiRegs,
    pub si: SiRegs,
    pub dp: DpRegs,
    pub mi: MiRegs,
}

/// Whether `offset` (already in `RdramAddr`-offset space -- i.e.
/// `RdramAddr::offset()`'s return value, `real_address -
/// 0xFFFFFFFF80000000`, the same space `Rdram`'s own accessors index into)
/// falls inside the MMIO window this module owns, vs. a plain RDRAM offset
/// `Rdram` should handle. See `RDRAM_MMIO_WINDOW_START`/`_END`'s doc
/// comments for the exact translated bounds; anything in-window but not
/// decoded by `MmioSpace::read_w`/`write_w` falls through to that
/// function's own loud trap rather than a silent wraparound into `Rdram`.
pub fn is_mmio_offset(offset: u32) -> bool {
    (RDRAM_MMIO_WINDOW_START..RDRAM_RCP_MMIO_END).contains(&offset)
}

// `base::UNIT | 0x00` below is deliberate, uniform "base + register offset"
// notation matching every other arm's `base::UNIT | 0xNN` shape (kept
// consistent rather than special-casing offset 0 as a bare `base::UNIT`) --
// the `identity_op` lint doesn't know the `0x00` is documentation of the
// real register's offset within its block, not a leftover no-op.
#[allow(clippy::identity_op)]
impl MmioSpace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Word read, decoded by hardware-unit base + register offset.
    /// `window_offset` is relative to `RDRAM_MMIO_WINDOW_START` (i.e.
    /// `RdramAddr::offset() - RDRAM_MMIO_WINDOW_START`) -- callers resolve
    /// the raw KSEG1 address to an `RdramAddr` first (`RdramAddr`'s own
    /// existing "translate once, pass a resolved offset" convention), then
    /// subtract this module's window base before calling in.
    pub fn read_w(&mut self, window_offset: u32) -> u32 {
        match window_offset {
            o if o == base::AI | 0x0C => self.ai.status(),
            o if o == base::AI | 0x04 => self.ai.length(),
            o if o == base::AI | 0x00 => self.ai.dram_addr,
            o if o == base::AI | 0x08 => self.ai.control,
            o if o == base::AI | 0x10 => self.ai.dacrate,
            o if o == base::AI | 0x14 => self.ai.bitrate,

            o if o == base::VI | 0x10 => self.vi.current_line,
            o if o == base::VI | 0x00 /* VI_STATUS shares offset 0 with mode in some maps */ => self.vi.status,

            o if o == base::PI | 0x00 => self.pi.dram_addr,
            o if o == base::PI | 0x04 => self.pi.cart_addr,
            o if o == base::PI | 0x10 => self.pi.status,

            o if o == base::SI | 0x18 => self.si.status,

            o if o == base::DP_CMD | 0x00 => self.dp.start,
            o if o == base::DP_CMD | 0x04 => self.dp.end,
            o if o == base::DP_CMD | 0x08 => self.dp.current,
            o if o == base::DP_CMD | 0x0C => self.dp.status,

            o if o == base::MI | 0x00 => self.mi.mode,
            o if o == base::MI | 0x0C => self.mi.intr_mask,
            o if o == base::MI | 0x08 => self.mi.intr,

            _ => panic!(
                "MmioSpace::read_w: unmodeled hardware register at rdram-relative offset \
                 {window_offset:#010x} (real KSEG1 address {:#010x}) -- add it to this module's \
                 decode table rather than silently returning 0, per AGENTS.md's \"loud traps, no \
                 silent shrugs\"",
                (RDRAM_MMIO_WINDOW_START + window_offset).wrapping_add(0x8000_0000)
            ),
        }
    }

    /// Word write, same `window_offset` convention as `read_w`.
    pub fn write_w(&mut self, window_offset: u32, value: u32) {
        match window_offset {
            o if o == base::AI | 0x00 => self.ai.dram_addr = value,
            o if o == base::AI | 0x04 => self.ai.set_next_buffer(self.ai.dram_addr, value),
            o if o == base::AI | 0x08 => self.ai.control = value,
            o if o == base::AI | 0x0C => { /* AI_STATUS write clears the AI interrupt; no interrupt line modeled yet, so this is a documented no-op */
            }
            o if o == base::AI | 0x10 => self.ai.dacrate = value,
            o if o == base::AI | 0x14 => self.ai.bitrate = value,

            o if o == base::VI | 0x10 => { /* Any write acknowledges VI in the integrated interrupt fabric. */
            }

            o if o == base::PI | 0x00 => self.pi.dram_addr = value,
            o if o == base::PI | 0x04 => self.pi.cart_addr = value,
            o if o == base::PI | 0x10 => self.pi.status = 0, // any write to PI_STATUS clears busy/error bits, per hardware docs

            o if o == base::SI | 0x18 => { /* Any write acknowledges SI in the integrated interrupt fabric. */
            }

            o if o == base::DP_CMD | 0x00 => self.dp.start = value,
            o if o == base::DP_CMD | 0x04 => self.dp.end = value,
            o if o == base::DP_CMD | 0x0C => self.dp.apply_status_command(value),

            o if o == base::MI | 0x00 => self.mi.apply_mode_command(value),
            o if o == base::MI | 0x0C => self.mi.apply_mask_command(value),
            o if o == base::MI | 0x08 => { /* MI_INTR is read-only on real hardware; write is documented as ignored */
            }

            _ => panic!(
                "MmioSpace::write_w: unmodeled hardware register at rdram-relative offset \
                 {window_offset:#010x} (real KSEG1 address {:#010x}, value {value:#010x}) -- add \
                 it to this module's decode table rather than silently dropping the write, per \
                 AGENTS.md's \"loud traps, no silent shrugs\"",
                (RDRAM_MMIO_WINDOW_START + window_offset).wrapping_add(0x8000_0000)
            ),
        }
    }

    /// The exact set of rdram-relative offsets (window-relative, i.e. what
    /// `read_w`/`write_w` accept) this module currently decodes -- used by
    /// `sync_into_rdram` to know which bytes of the backing buffer to keep
    /// live, without hand-maintaining a second list that could drift out of
    /// sync with `read_w`'s own match arms.
    fn modeled_offsets(&self) -> [u32; 18] {
        [
            base::AI | 0x00,
            base::AI | 0x04,
            base::AI | 0x08,
            base::AI | 0x0C,
            base::AI | 0x10,
            base::AI | 0x14,
            base::VI | 0x10,
            base::VI | 0x00,
            base::PI | 0x00,
            base::PI | 0x04,
            base::PI | 0x10,
            base::SI | 0x18,
            base::DP_CMD | 0x00,
            base::DP_CMD | 0x04,
            base::DP_CMD | 0x08,
            base::DP_CMD | 0x0C,
            base::MI | 0x00,
            base::MI | 0x0C,
        ]
    }

    /// Write every modeled register's CURRENT value into `rdram`'s real
    /// bytes at the corresponding `RDRAM_MMIO_WINDOW_START`-relative
    /// offset, in the SAME native-endian word format `Rdram::write_w`/the
    /// generated `MEM_W` macro use (see `rdram.rs`'s module doc correction:
    /// `MEM_W` is a native-endian, not big-endian, word dereference) --
    /// this is what makes a raw guest `lw` at e.g. `AI_STATUS`'s address
    /// observe the modeled value instead of stale/garbage bytes.
    ///
    /// Call this whenever a register's host-visible value changes AND
    /// before any point generated code might issue a raw MMIO load (in
    /// practice: right after any host mutation of `self`, and once before
    /// resuming a coroutine -- see `AiRegs::status`'s one-shot-busy comment
    /// for why AI_STATUS specifically should be synced immediately before
    /// a guest read, not lazily).
    ///
    /// # Safety
    /// `rdram` must point to a buffer of at least `RDRAM_MMIO_WINDOW_END`
    /// bytes (the caller -- `Rdram::new`/a harness's own allocation -- is
    /// responsible for sizing the buffer that large; see module doc).
    ///
    /// Deliberately one-directional (model -> rdram bytes only, no
    /// `sync_from_rdram` counterpart). Typed-Rust recompiled `lw/sw` now call
    /// `read_w`/`write_w` directly through `fn64-recomp-rs`'s MMIO hooks, so
    /// their side effects cannot be re-derived or double-applied from bytes.
    /// The byte mirror remains the compatibility path for generated C, whose
    /// raw pointer macros cannot be intercepted; retiring that one-directional
    /// C-lane bridge requires the universal block/dynamic execution lane.
    pub unsafe fn sync_into_rdram(&mut self, rdram: *mut u8) {
        for window_offset in self.modeled_offsets() {
            let value = self.read_w(window_offset);
            let addr = (RDRAM_MMIO_WINDOW_START + window_offset) as usize;
            unsafe {
                std::ptr::copy_nonoverlapping(value.to_ne_bytes().as_ptr(), rdram.add(addr), 4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_status_read_does_not_deadlock_a_polling_loop() {
        // The exact failure mode the task names: a raw AI_STATUS read must
        // return a value with AI_STATUS_BUSY/FULL both clear so `while
        // (osAiGetStatus() & (AI_STATUS_BUSY|AI_STATUS_FULL)) {}` proceeds.
        let mut mmio = MmioSpace::new();
        let status = mmio.read_w(base::AI | 0x0C);
        assert_eq!(
            status & AI_STATUS_FULL,
            0,
            "AI must never report FULL by default"
        );
        assert_eq!(status, 0, "idle AI reports not-busy on a fresh read");
    }

    #[test]
    fn ai_set_next_buffer_then_status_reports_busy_once() {
        let mut mmio = MmioSpace::new();
        mmio.write_w(base::AI, 0x1234); // AI_DRAM_ADDR is at +0x00
        mmio.write_w(base::AI | 0x04, 0x40); // length -> triggers set_next_buffer
        let first = mmio.read_w(base::AI | 0x0C);
        assert_eq!(
            first & AI_STATUS_BUSY,
            AI_STATUS_BUSY,
            "first status read after a submit observes busy"
        );
        let second = mmio.read_w(base::AI | 0x0C);
        assert_eq!(
            second, 0,
            "busy is one-shot -- proceeds rather than spinning forever"
        );
    }

    #[test]
    fn dp_status_commands_update_latched_state_not_command_bits() {
        let mut dp = DpRegs::default();
        dp.apply_status_command(0x02 | 0x08 | 0x20);
        assert_eq!(
            dp.status,
            DP_STATUS_XBUS_DMA | DP_STATUS_FREEZE | DP_STATUS_FLUSH
        );
        dp.apply_status_command(0x01 | 0x04 | 0x10);
        assert_eq!(dp.status, 0);
    }

    #[test]
    fn ai_get_length_reports_latched_length() {
        let mut mmio = MmioSpace::new();
        mmio.write_w(base::AI | 0x04, 0x100);
        assert_eq!(mmio.read_w(base::AI | 0x04), 0x100);
    }

    #[test]
    fn pi_status_write_clears_busy() {
        let mut mmio = MmioSpace::new();
        mmio.pi.status = 0b11;
        mmio.write_w(base::PI | 0x10, 0);
        assert_eq!(mmio.read_w(base::PI | 0x10), 0);
    }

    #[test]
    fn is_mmio_offset_recognizes_the_translated_hw_window() {
        // Real crash case: RdramAddr::from_gpr(0xA450000C) == 0x2450000C
        // (see rdram.rs's KSEG0_BASE_SIGN_EXTENDED and this module's doc
        // comment citing docs/BOOT-NOTES-WM2000.md's exact LLDB evidence).
        assert!(is_mmio_offset(
            crate::RdramAddr::from_gpr(0xA450_000C).offset()
        ));
        assert!(is_mmio_offset(RDRAM_MMIO_WINDOW_START));
        assert!(is_mmio_offset(
            crate::RdramAddr::from_gpr(0xA480_0018).offset()
        ));
        assert!(!is_mmio_offset(RDRAM_RCP_MMIO_END));
        assert!(
            !is_mmio_offset(crate::RdramAddr::from_gpr(0xA600_1010).offset()),
            "KSEG1 cartridge space is not RCP MMIO"
        );
        assert!(
            !is_mmio_offset(0x0000_1000),
            "a plain low rdram offset is not MMIO"
        );
        assert!(
            !is_mmio_offset(RDRAM_MMIO_WINDOW_END),
            "past the documented hw window"
        );
    }

    #[test]
    #[should_panic(expected = "unmodeled hardware register")]
    fn unmodeled_register_traps_loudly_not_silently() {
        let mut mmio = MmioSpace::new();
        // AI base + some far offset with no modeled register.
        mmio.read_w(base::AI | 0xFF);
    }

    /// The real bug this module fixes end to end: a raw guest `lw` at
    /// `AI_STATUS` (`0xA450000C`) must land on a real, in-bounds,
    /// correctly-valued byte -- not panic on an out-of-bounds `Rdram`
    /// access (`docs/BOOT-NOTES-WM2000.md`'s exact LLDB-confirmed crash).
    #[test]
    fn sync_into_rdram_backs_a_raw_guest_ai_status_load() {
        let mut mmio = MmioSpace::new();
        let mut buf = vec![0u8; RDRAM_MMIO_WINDOW_END as usize];

        // Simulate: osAiSetNextBuffer-equivalent host mutation happened,
        // then sync before the guest's raw load.
        mmio.ai.set_next_buffer(0x1000, 0x40);
        unsafe { mmio.sync_into_rdram(buf.as_mut_ptr()) };

        // Real guest address 0xA450000C -> RdramAddr::from_gpr ->
        // 0x2450000C, exactly what docs/BOOT-NOTES-WM2000.md's LLDB
        // backtrace computed.
        let ai_status_addr = crate::RdramAddr::from_gpr(0xA450_000C);
        assert_eq!(ai_status_addr.offset(), 0x2450_000C);
        let o = ai_status_addr.offset() as usize;
        let raw = i32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
        assert_eq!(
            raw as u32 & AI_STATUS_BUSY,
            AI_STATUS_BUSY,
            "a raw MEM_W read at the real crash address must observe the modeled busy bit"
        );
    }
}
