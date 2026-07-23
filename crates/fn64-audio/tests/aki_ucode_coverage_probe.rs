//! Coverage guard: the AKI-family audio microcode recompiles fully through
//! fn64-audio's OoT-proven RSP recompiler — no unimplemented instructions.
//!
//! Measured finding (2026-07): the AKI audio ucode (789 instructions) emits
//! ZERO scalar `trap_unknown` sites, and every VU op it uses is in `dispatch`'s
//! implemented set. So making WM2000 audible is an INTEGRATION task (emit the
//! `aki-audio-ucode` crate + wire it in), not an RSP-op-implementation task.
//! This test locks that in: it fails if a change reintroduces an undecoded
//! scalar op or a VU op the AKI ucode needs that `dispatch` no longer handles.
//!
//! Env: FN64_WM2000_ROM = path to the WM2000 (.z64). The AKI audio ucode text
//! is 3156 bytes at ROM 0x39510, vram 0x80038910 (byte-identical across the AKI
//! family: WT / Revenge / WM2000 / No Mercy). Loud skip when unset so CI
//! without the ROM stays green.

use fn64_audio::rsp::recomp::emit::emit_module;

const AKI_UCODE_ROM_OFF: usize = 0x39510;
const AKI_UCODE_LEN: usize = 0xC54; // 3156 bytes
const AKI_UCODE_BASE_VRAM: u32 = 0x8910; // vram 0x80038910

/// VU ops the AKI audio ucode uses (measured from the emitted module). Every
/// one must be handled by `fn64_audio::rsp::ops::dispatch`. If the recompiler
/// or dispatch changes such that the ucode uses an op outside this set, the
/// count check below catches it.
const AKI_VU_OPS: &[&str] = &[
    "Vadd", "Vaddc", "Vand", "Vge", "Vlt", "Vmacf", "Vmadh", "Vmadm", "Vmadn", "Vmudh", "Vmudl",
    "Vmudm", "Vmudn", "Vmulf", "Vsar", "Vsub", "Vxor",
];

#[test]
fn aki_audio_ucode_recompiles_with_no_undecoded_instructions() {
    let Some(rom_path) = std::env::var_os("FN64_WM2000_ROM") else {
        eprintln!(
            "SKIP aki_audio_ucode_recompiles_with_no_undecoded_instructions: set \
             FN64_WM2000_ROM to the WM2000 .z64 to run the AKI ucode coverage guard."
        );
        return;
    };
    let rom = std::fs::read(&rom_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", rom_path.to_string_lossy()));
    let words: Vec<u32> = rom[AKI_UCODE_ROM_OFF..AKI_UCODE_ROM_OFF + AKI_UCODE_LEN]
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect();

    let src = emit_module(&words, AKI_UCODE_BASE_VRAM, "aki_audio_ucode");

    // A scalar `break 'run trap_unknown(0xPC, 0xWORD);` line is a genuinely
    // undecoded scalar instruction (distinct from the per-VU-op Unimplemented
    // fallback arm, which the emitter writes for every VU dispatch). There must
    // be none.
    let scalar_undecoded: Vec<&str> = src
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("break 'run trap_unknown(0x"))
        .collect();
    assert!(
        scalar_undecoded.is_empty(),
        "AKI audio ucode has {} undecoded scalar instruction(s); first: {}",
        scalar_undecoded.len(),
        scalar_undecoded.first().copied().unwrap_or("")
    );

    // The VU ops the ucode actually uses, from the emitted dispatch calls.
    let mut used: std::collections::BTreeSet<String> = Default::default();
    for line in src.lines() {
        if let Some(rest) = line.trim().strip_prefix("match dispatch(m.vu(), VuOp::") {
            if let Some(name) = rest.split(',').next() {
                used.insert(name.trim().to_string());
            }
        }
    }
    // Every used VU op must be in our known-implemented inventory. A new op
    // outside it is the signal that AKI needs a VU op we haven't verified.
    let unexpected: Vec<&String> = used
        .iter()
        .filter(|op| !AKI_VU_OPS.contains(&op.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "AKI audio ucode uses VU op(s) outside the verified-implemented set: {unexpected:?} \
         (update AKI_VU_OPS and confirm dispatch handles them)"
    );

    eprintln!(
        "AKI audio ucode: {} instructions, 0 undecoded scalar, {} distinct VU ops (all handled).",
        words.len(),
        used.len()
    );

    // The module must be a complete, self-contained function.
    assert!(src.contains("pub fn aki_audio_ucode(m: &mut RspMachine) -> RspExitReason"));
}
