//! `fn64-diff`: the lockstep/state-transplant harness.
//!
//! Turns a reference mupen64plus savestate (produced by the faki-tools
//! oracle, or mupen64plus's own GUI) into inputs fn64 can execute from:
//! full RDRAM contents plus CPU register state (GPRs, CP0, hi/lo, resume
//! PC). See `savestate` for the parser and its provenance/finding writeup.
//!
//! ## Why this exists
//!
//! Every prior fn64 milestone proved "boots from cold reset, executes N
//! steps, may or may not reach steady state." This crate decouples
//! "executes correctly" from "boots correctly": if a mid-game reference
//! snapshot runs forward on fn64 without immediately diverging, that is
//! independent evidence the executor/ABI/instruction semantics are sound,
//! even while boot itself is still being climbed. It is also the
//! foundation the lockstep-vs-reference-emulator differential harness
//! builds on (same snapshot format, same RDRAM layout question) -- see
//! [`oracle_client`] (subprocess client for the faki-tools oracle binary)
//! and [`lockstep`] (the first-divergence comparison engine), and
//! `bin/lockstep.rs` for the end-to-end runnable harness.
//!
//! ## The real architectural finding: no mid-function resume
//!
//! fn64 (like N64Recomp itself) compiles each MIPS function to one C
//! function; `SectionRegistry::resolve` (`fn64-runtime/src/overlay.rs`)
//! only matches a vram address that is an EXACT function-entry offset, by
//! design (`LOOKUP_FUNC`'s only real call shape: a whole-function indirect
//! call). A snapshot's saved PC lands wherever an instruction happened to
//! be executing, essentially never exactly at a function's first
//! instruction. So true instruction-exact transplant ("resume at PC") is
//! not just unimplemented here -- it is not representable by a
//! recompiler-shaped runtime at all without either (a) sub-function-
//! granularity call targets (which N64Recomp's own codegen does not
//! produce) or (b) a bytecode/threaded interpreter fallback for the
//! remainder of the interrupted function. This crate is honest about that:
//! [`resolve_entry_point`] finds the ENCLOSING function (the nearest
//! registered function whose vram range contains the resume PC) and
//! reports the offset into it, rather than silently pretending an exact
//! resume happened.

pub mod lockstep;
pub mod oracle_client;
pub mod savestate;

use fn64_runtime::{RdramAddr, Rdram};

pub use savestate::{ParseError, Snapshot, CP0_CAUSE, CP0_EPC, CP0_STATUS, GPR_NAMES};

/// Convert a snapshot's raw (N64/big-endian-instruction-stream) RDRAM bytes
/// into an `fn64_runtime::Rdram` using its native-word convention.
///
/// `fn64_runtime::rdram`'s module doc (verified against a real boot's stack
/// pointer, see that module's "Correction (byte order)" section) is
/// explicit that `Rdram::read_w` etc. are plain native-endian pointer
/// dereferences with NO byte-swap of their own -- exactly matching real
/// generated `MEM_W`. That means the BACKING BYTES must already be stored
/// word-swapped relative to a raw N64/ROM/savestate memory image on a
/// little-endian host, or a real `lw` of a known big-endian instruction
/// word would silently come back byte-reversed. This function performs
/// that one swap explicitly, once, at the ingestion boundary, rather than
/// asking every future caller to remember it.
///
/// This is the one adaptation this crate makes to source data whose real
/// runtime origin (the recompiler's own ROM-loading step) was not
/// available to inspect directly for this task's time-boxed scope -- flagged
/// here rather than silently assumed. See `bin/dump_snapshot.rs`'s report
/// output for the honest caveat surfaced to the operator.
pub fn to_fn64_rdram(snapshot: &Snapshot) -> Rdram {
    let mut rdram = Rdram::new(snapshot.rdram.len().max(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE));
    let mut swapped = snapshot.rdram.clone();
    for word in swapped.chunks_exact_mut(4) {
        word.swap(0, 3);
        word.swap(1, 2);
    }
    rdram.write_bytes(0, &swapped);
    rdram
}

/// Seed a fresh `fn64_abi::RecompContext`-shaped register file from a
/// snapshot. Returns `(gprs_as_u64, hi, lo)` rather than constructing
/// `fn64_abi::RecompContext` directly, since `fn64-diff` deliberately does
/// not depend on `fn64-abi` (keeping this crate's core parsing/converting
/// logic testable without pulling in the ABI/executor/render/audio
/// dependency chain) -- the thin `fn64-abi`-aware glue (a handful of field
/// assignments) belongs in the example harness that actually boots a game,
/// per `docs/DESIGN.md`'s crate-layering rule ("fn64-runtime has no
/// knowledge of fn64-abi's extern C surface").
pub fn seed_registers(snapshot: &Snapshot) -> ([u64; 32], u64, u64) {
    (snapshot.gprs, snapshot.mult_hi, snapshot.mult_lo)
}

/// Read a word out of a snapshot's raw RDRAM at a vram address, without
/// going through the fn64 `Rdram`/`RdramAddr` conversion -- for inspecting
/// snapshot content directly (e.g. verifying the resume PC's enclosing
/// bytes look like a plausible instruction stream) independent of whether
/// `to_fn64_rdram`'s swap convention is exactly right.
pub fn read_raw_be_word(snapshot: &Snapshot, vram: u32) -> Option<u32> {
    let offset = vram.checked_sub(0x8000_0000)? as usize;
    let bytes = snapshot.rdram.get(offset..offset + 4)?;
    Some(u32::from_be_bytes(bytes.try_into().unwrap()))
}

/// Result of resolving a snapshot's resume PC against a set of known
/// function entry points (vram, size) pairs -- see module doc's "no
/// mid-function resume" finding. `entries` need not be sorted; this does a
/// linear scan (fine for the tens-to-low-thousands of functions a single
/// NW4E overlay section has -- not a hot path).
#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedEntry {
    /// The resume PC is exactly a registered function's first instruction
    /// -- genuine, exact transplant is possible.
    ExactEntry { vram: u32 },
    /// The resume PC falls inside a registered function's range but NOT at
    /// its entry -- transplant can only start the ENCLOSING function from
    /// its own top, which is a materially different (and likely
    /// incorrect-for-this-invocation) execution than truly resuming
    /// mid-body. Reported honestly, not silently substituted.
    EnclosingFunction { entry_vram: u32, offset_into_fn: u32 },
    /// No registered function's range contains the resume PC at all.
    NotFound,
}

pub fn resolve_entry_point(resume_pc: u32, entries: &[(u32, u32)]) -> ResolvedEntry {
    for &(entry_vram, size) in entries {
        if resume_pc == entry_vram {
            return ResolvedEntry::ExactEntry { vram: entry_vram };
        }
        if resume_pc > entry_vram && resume_pc < entry_vram.wrapping_add(size) {
            return ResolvedEntry::EnclosingFunction {
                entry_vram,
                offset_into_fn: resume_pc - entry_vram,
            };
        }
    }
    ResolvedEntry::NotFound
}

/// Convenience: build an `RdramAddr` for a vram address using the same
/// KSEG0-subtraction convention every `MEM_*` accessor and `fn64-abi` shim
/// uses (`fn64_runtime::RdramAddr::from_gpr`), for callers of this crate
/// that want to peek/poke the converted `Rdram` at a known vram location
/// (e.g. re-reading the resume PC's instruction word post-conversion to
/// sanity-check the swap).
pub fn rdram_addr(vram: u32) -> RdramAddr {
    RdramAddr::from_gpr(vram as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fn64_rdram_swaps_each_word_so_native_read_w_recovers_the_be_value() {
        let mut snap_bytes = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        // A real big-endian MIPS instruction word: `lui k0, 0x8004` is
        // 3C 1A 80 04 (verified against the oracle's own `breakpoint` dump
        // of a real fixture's boot-vector bytes at 0x80000000).
        snap_bytes[0..4].copy_from_slice(&[0x3C, 0x1A, 0x80, 0x04]);
        let snapshot = Snapshot {
            version: 0x0001_0900,
            rom_md5: String::new(),
            pc: 0x8000_1000,
            gprs: [0u64; 32],
            cp0: [0u32; 32],
            mult_lo: 0,
            mult_hi: 0,
            rdram: snap_bytes,
        };

        let rdram = to_fn64_rdram(&snapshot);
        let word = rdram.read_w(rdram_addr(0x8000_0000));
        // Native (little-endian host) read of the byte-swapped word must
        // reconstruct the original big-endian instruction's numeric value.
        assert_eq!(word as u32, 0x3C1A_8004);
    }

    #[test]
    fn seed_registers_passes_through_snapshot_values_unchanged() {
        let mut gprs = [0u64; 32];
        gprs[29] = 0x8005_6ff0;
        let snapshot = Snapshot {
            version: 0,
            rom_md5: String::new(),
            pc: 0,
            gprs,
            cp0: [0u32; 32],
            mult_lo: 0x1122,
            mult_hi: 0x3344,
            rdram: vec![0u8; 16],
        };
        let (out_gprs, hi, lo) = seed_registers(&snapshot);
        assert_eq!(out_gprs[29], 0x8005_6ff0);
        assert_eq!(hi, 0x3344);
        assert_eq!(lo, 0x1122);
    }

    #[test]
    fn resolve_entry_point_exact_match() {
        let entries = [(0x8000_1000, 0x40), (0x8000_2000, 0x100)];
        assert_eq!(
            resolve_entry_point(0x8000_1000, &entries),
            ResolvedEntry::ExactEntry { vram: 0x8000_1000 }
        );
    }

    #[test]
    fn resolve_entry_point_enclosing_function_is_reported_not_hidden() {
        let entries = [(0x8000_1000, 0x40)];
        assert_eq!(
            resolve_entry_point(0x8000_1020, &entries),
            ResolvedEntry::EnclosingFunction {
                entry_vram: 0x8000_1000,
                offset_into_fn: 0x20,
            }
        );
    }

    #[test]
    fn resolve_entry_point_not_found() {
        let entries = [(0x8000_1000, 0x40)];
        assert_eq!(resolve_entry_point(0x8009_9999, &entries), ResolvedEntry::NotFound);
    }
}
